//! The ten tickable sound instances — vanilla's per-tick volume, pitch and
//! position ramps (M141).
//!
//! `SoundEngine.tickInGameSound` walks `tickingSounds`, calls `instance.tick()`
//! on each, and pushes the resulting volume/pitch/position at the channel
//! (`SoundEngine.java:234-253`). M131 built that loop and M138a gave it a
//! device; what it drove was one `tick()` body out of ten, because
//! `EntityBoundSoundInstance` was the only subclass Rewo modelled. This module
//! is the other nine, each transcribed line by line.
//!
//! # There are ten, not "~8"
//!
//! `REWO_AUDIO_PLAN.md` §M140 says "the ~8 tickable per-tick ramps". Counted
//! from `grep -rn 'extends AbstractTickableSoundInstance\|extends
//! BeeSoundInstance\|extends RidingEntitySoundInstance'` over the decompile,
//! the classes with a distinct `tick()` body are **ten**:
//! `EntityBoundSoundInstance`, `BeeSoundInstance`,
//! `BiomeAmbientSoundsHandler.LoopSoundInstance`, `DirectionalSoundInstance`,
//! `ElytraOnPlayerSoundInstance`, `GuardianAttackSoundInstance`,
//! `MinecartSoundInstance`, `RidingEntitySoundInstance`,
//! `SnifferSoundInstance`, and the two nested in
//! `UnderwaterAmbientSoundInstances` — which is eleven bodies if you count
//! both of those, ten if you count `SubSound` and the loop as one file. The
//! enum below has one variant per body. `BeeFlyingSoundInstance` /
//! `BeeAggressiveSoundInstance` and `RidingMinecartSoundInstance` are *not*
//! extra bodies: they override protected hooks, which are fields here.
//!
//! # The finding: a minecart's pitch ramp is dead code in vanilla
//!
//! `MinecartSoundInstance.java:16` declares
//!
//! ```java
//! private float pitch = 0.0F;
//! ```
//!
//! and `AbstractSoundInstance.java:16` already declares
//! `protected float pitch = 1.0F;`. **Java field access is statically bound**,
//! so `AbstractSoundInstance.getPitch()` — the only reader, and the one
//! `SoundEngine.calculatePitch` calls through the `SoundInstance` interface —
//! resolves `this.pitch` to the *superclass* field, which
//! `MinecartSoundInstance` never writes. Its `tick()` writes the shadow.
//!
//! So `PITCH_MIN = 0.0F`, `PITCH_MAX = 1.0F` and `PITCH_DELTA = 0.0025F` name
//! a ramp that reaches nothing: a riding minecart's pitch is a constant 1.0
//! multiplied by the `sounds.json` entry's own pitch, from the first tick to
//! the last. It is the only field shadow in the whole
//! `client/resources/sounds` package — checked, not assumed.
//!
//! This is worth the paragraph because the *plausible* transcription is the
//! wrong one. Reading `MinecartSoundInstance.java` on its own gives a minecart
//! whose pitch starts at 0 (clamped by `calculatePitch` to 0.5, a half-speed
//! rumble) and sweeps to 1.0 over 400 ticks — twenty seconds of audible
//! glissando at the start of every minecart ride, which vanilla does not have.
//! The shadow is reproduced below, under a name that says what it is, so the
//! ramp is visible and its deadness is testable.
//!
//! # Three more places the plausible reading diverges
//!
//! **`Mth.lerp` takes a factor, and two of these classes clamp it to the
//! output range instead of to `0..1`.** `Mth.lerp(alpha, p0, p1)` is
//! `p0 + alpha * (p1 - p0)` (`Mth.java:550`). The bee writes
//!
//! ```java
//! this.volume = Mth.lerp(Mth.clamp(speed, 0.0F, 0.5F), 0.0F, 1.2F);
//! ```
//!
//! so the factor saturates at **0.5** and the volume at `0.5 * 1.2 = 0.6` —
//! half the `VOLUME_MAX = 1.2F` the class declares. The minecart does the same
//! with `0.7F`, so its ceiling is `0.35`. A transcription that reads the named
//! constant as the ceiling makes both sounds twice as loud as vanilla.
//!
//! The bee's *pitch* line is stranger still:
//!
//! ```java
//! this.pitch = Mth.lerp(Mth.clamp(speed, this.getMinPitch(), this.getMaxPitch()),
//!                       this.getMinPitch(), this.getMaxPitch());
//! ```
//!
//! The factor is clamped to the **pitch range**, so for an adult bee it never
//! leaves `[0.7, 1.1]` and the result never leaves `[0.98, 1.14]`. A bee's
//! horizontal speed does not reach 0.7, so in practice an adult bee's pitch is
//! the constant **0.98** and a baby's the constant **1.54** — never the
//! `0.7..1.1` / `1.1..1.5` bands the getters describe. Clamping to `0..1`
//! "fixes" it into something vanilla does not do.
//!
//! **`RidingEntitySoundInstance` uses `Mth.clampedLerp`, which is a different
//! function.** `clampedLerp` clamps the *factor* to `0..1` (`Mth.java:118`),
//! so its speed-to-volume map is not the bee's with different numbers. Two
//! adjacent classes, both mapping "speed" to "volume", by two formulas that do
//! not agree anywhere except at zero.
//!
//! **`Mob.getTarget()` is never synchronised, so on a client it is always
//! null.** The guardian and the sniffer both gate their ramp on
//! `entity.getTarget() == null`. `Mob.target` is a plain field written by AI
//! goals, which run in `serverAiStep`; there is no `EntityDataAccessor` for it
//! anywhere. The guardian's guard therefore reduces to `!isRemoved()` on any
//! real client, and a transcription that maps `getTarget()` onto the *synced*
//! `DATA_ID_ATTACK_TARGET` (which is what drives `clientSideAttackTime`, one
//! method away) would silence the sound exactly while it should be playing.
//! [`RampWorld::has_ai_target`] exists, is asked, and is documented as
//! always-false rather than deleted, because the deletion would be a guess and
//! the constant is a fact.
//!
//! # Two structural asymmetries
//!
//! **`BiomeAmbientSoundsHandler.LoopSoundInstance.tick` has no `else`.** Every
//! other body here is `if (alive) { ramp } else { stop(); }`; that one is
//!
//! ```java
//! if (this.fade < 0) { this.stop(); }
//! this.fade = this.fade + this.fadeDirection;
//! this.volume = Mth.clamp(this.fade / 40.0F, 0.0F, 1.0F);
//! ```
//!
//! so a stopped instance still advances its fade and still writes a volume.
//! And its `fadeDirection` starts at **0**, so a freshly built one never moves
//! until `fadeIn()`/`fadeOut()` is called.
//!
//! **The underwater loop fades in at +1 and out at −2.** `fade++` while
//! submerged, `fade -= 2` while not, capped above at 40 and *not* capped
//! below — so surfacing silences it in twenty ticks where submerging takes
//! forty, and the `fade >= 0` guard is read *before* the update, so it stops
//! on the tick after `fade` goes negative rather than the tick it does.
//!
//! # What this module is not
//!
//! It does not construct these instances — that is the trigger half, and the
//! triggers are `handleAddEntity`'s `postAddEntitySoundInstance`,
//! `handleEntityEvent` cases 21 and 63, `LocalPlayer.startRiding`, and
//! `LocalPlayer.onSyncedDataUpdated`'s fall-flying rising edge. Nor does it
//! open a device. It is the arithmetic, which is the part a machine can grade.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! `net/minecraft/client/resources/sounds/` — `AbstractSoundInstance.java`,
//! `AbstractTickableSoundInstance.java`, `BeeSoundInstance.java`,
//! `BeeFlyingSoundInstance.java`, `BeeAggressiveSoundInstance.java`,
//! `BiomeAmbientSoundsHandler.java`, `DirectionalSoundInstance.java`,
//! `ElytraOnPlayerSoundInstance.java`, `EntityBoundSoundInstance.java`,
//! `GuardianAttackSoundInstance.java`, `MinecartSoundInstance.java`,
//! `RidingEntitySoundInstance.java`, `RidingMinecartSoundInstance.java`,
//! `SnifferSoundInstance.java`, `UnderwaterAmbientSoundInstances.java`; plus
//! `util/Mth.java`, `world/phys/Vec3.java`, `world/entity/NeutralMob.java`,
//! `world/entity/monster/Guardian.java`, `world/entity/animal/bee/Bee.java`.

use crate::sound_instance::{mth_clamp, Attenuation, SoundInstance};
use crate::sounds::SoundSource;

// ---------------------------------------------------------------------------
// `Mth` — the two functions this module needs that `sound_instance` does not
// already have. `mth_clamp` is reused rather than copied: a second copy of the
// same arithmetic is a second chance to drift, and its `NaN` behaviour (Java's
// `Math.min` propagates `NaN` where Rust's `f32::min` swallows it) is already
// pinned by a test there.
// ---------------------------------------------------------------------------

/// `Mth.lerp(float alpha, float p0, float p1)` — `Mth.java:550`.
///
/// Note the argument order: the **factor comes first**. Vanilla's own callers
/// in this module pass an already-clamped speed as that factor, which is where
/// the two ceiling quirks in the module doc come from.
#[inline]
pub fn mth_lerp(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

/// `Mth.clampedLerp(float factor, float min, float max)` — `Mth.java:118`.
///
/// **Not** `mth_lerp(mth_clamp(f, min, max), min, max)`. This clamps the
/// factor to `0..1`; that clamps it to the output range. They agree only at
/// `factor == 0`, and `RidingEntitySoundInstance` uses this one while the bee
/// and the minecart use the other.
#[inline]
pub fn mth_clamped_lerp(factor: f32, min: f32, max: f32) -> f32 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        mth_lerp(factor, min, max)
    }
}

/// `NeutralMob.isAngry()` — `NeutralMob.java:112-120`.
///
/// Two conditions, and the first is easy to drop: `endTime > 0` *then*
/// `endTime - gameTime > 0`. `Bee` seeds `DATA_ANGER_END_TIME` to **-1**
/// (`Bee.java:165`), so the first test is what makes a never-angered bee calm
/// rather than "angry until game time -1". Both are strict.
///
/// It is a synced deadline rather than a flag, which is why a client can
/// answer it at all: the bee's flying/aggressive sound switch reads this every
/// tick.
#[inline]
pub fn is_angry(anger_end_time: i64, game_time: i64) -> bool {
    anger_end_time > 0 && anger_end_time - game_time > 0
}

/// `Guardian.getAttackAnimationScale(0.0F)` — `Guardian.java:288-290`.
///
/// `(clientSideAttackTime + partial) / getAttackDuration()`, where the
/// duration is **80 for a guardian and 60 for an elder guardian**
/// (`Guardian.java:109`, `ElderGuardian.java:40`) — so the same counter
/// produces a different scale per species, and the elder's ramp is a third
/// faster.
#[inline]
pub fn attack_animation_scale(client_side_attack_time: i32, attack_duration: i32) -> f32 {
    client_side_attack_time as f32 / attack_duration as f32
}

/// `Guardian.getAttackDuration()`.
pub const GUARDIAN_ATTACK_DURATION: i32 = 80;
/// `ElderGuardian.getAttackDuration()`.
pub const ELDER_GUARDIAN_ATTACK_DURATION: i32 = 60;

/// `ElytraOnPlayerSoundInstance.DELAY`.
pub const ELYTRA_DELAY: i32 = 20;
/// `UnderwaterAmbientSoundInstances.UnderwaterAmbientSoundInstance.FADE_DURATION`.
pub const UNDERWATER_FADE_DURATION: i32 = 40;

// ---------------------------------------------------------------------------
// The world seam
// ---------------------------------------------------------------------------

/// Everything the ten `tick()` bodies read from outside themselves.
///
/// One method per vanilla call, named after it, so a missing input is a
/// missing method rather than a plausible default buried in an implementation.
/// Two implementors are expected: the live adapter and a test fake.
///
/// The methods take an entity id because that is what Rewo has; vanilla holds
/// a strong reference to the `Entity` and so never asks "does it exist" —
/// [`RampWorld::position`] answering `None` is `entity.isRemoved()`.
pub trait RampWorld {
    /// The entity's position, or `None` when `entity.isRemoved()`.
    ///
    /// Vanilla reads the two separately; folding them is safe because every
    /// body here checks `isRemoved()` immediately before reading the position
    /// and does nothing else with it.
    fn position(&self, entity: i32) -> Option<(f64, f64, f64)>;

    /// `entity.getDeltaMovement().horizontalDistance()` — `sqrt(x² + z²)`.
    ///
    /// Returned as `f64` because vanilla's is a `double` and the `(float)`
    /// cast happens at the *call site*; doing it here would move a rounding
    /// step out of the place the decompile puts it.
    fn horizontal_speed(&self, entity: i32) -> f64;

    /// `entity.getDeltaMovement().length()` — `sqrt(x² + y² + z²)`.
    fn speed(&self, entity: i32) -> f64;

    /// `entity.getDeltaMovement().lengthSqr()` — `x² + y² + z²`, **not**
    /// rooted. `ElytraOnPlayerSoundInstance` divides this by 4 to get a
    /// volume, so a client that rooted it first is loud far too early.
    fn speed_sqr(&self, entity: i32) -> f64;

    /// `bee.isBaby()`.
    fn baby(&self, entity: i32) -> bool;

    /// `bee.isAngry()` — see [`is_angry`]; a synced deadline, not a flag.
    fn angry(&self, entity: i32) -> bool;

    /// `entity.isUnderWater()`.
    fn underwater(&self, entity: i32) -> bool;

    /// `Mob.getTarget() != null`.
    ///
    /// **A real client always answers `false`.** `Mob.target` is a plain field
    /// written by AI goals in `serverAiStep`, with no `EntityDataAccessor`
    /// anywhere, so it is never synchronised. The guardian and the sniffer
    /// both gate on it; for the guardian that makes the whole guard
    /// `!isRemoved()`. Kept as a question rather than folded away, because
    /// folding it would hide which of the two guards is doing the work.
    fn has_ai_target(&self, entity: i32) -> bool;

    /// `guardian.getAttackAnimationScale(0.0F)` — see
    /// [`attack_animation_scale`].
    fn attack_animation_scale(&self, entity: i32) -> f32;

    /// `sniffer.canPlayDiggingSound()` — `Sniffer.java:139-141`, which is
    /// `getState() == DIGGING || getState() == SEARCHING`. Two states, not
    /// one: a sniffer that has found something keeps the sound while it
    /// searches.
    fn sniffer_digging(&self, entity: i32) -> bool;

    /// `minecart.isOnRails()`.
    fn on_rails(&self, entity: i32) -> bool;

    /// `minecart.getBehavior() instanceof NewMinecartBehavior`.
    ///
    /// Both minecart ramps ask this, and both only *in conjunction with*
    /// `isOnRails()`: a cart on the old behaviour is never treated as off-rail.
    fn new_minecart_behavior(&self, entity: i32) -> bool;

    /// `level.tickRateManager().runsNormally()`. Only the minecart asks.
    fn runs_normally(&self) -> bool;

    /// `player.isFallFlying()`.
    fn fall_flying(&self, entity: i32) -> bool;

    /// `player.getVehicle()`, or `None` when `!player.isPassenger()`.
    ///
    /// `RidingEntitySoundInstance` tests `!isPassenger() || getVehicle() !=
    /// entity`, which this collapses to `vehicle_of(player) != Some(entity)` —
    /// exact, because `getVehicle()` is null precisely when `isPassenger()` is
    /// false.
    fn vehicle_of(&self, entity: i32) -> Option<i32>;

    /// `camera.position()` — only [`Ramp::Directional`] asks.
    fn camera_position(&self) -> (f64, f64, f64);
}

// ---------------------------------------------------------------------------
// The ramps
// ---------------------------------------------------------------------------

/// Which of the two bee loops an instance is.
///
/// The only difference between `BeeFlyingSoundInstance` and
/// `BeeAggressiveSoundInstance` is `shouldSwitchSounds()` (`isAngry()` vs
/// `!isAngry()`) and which class `getAlternativeSoundInstance()` builds, so it
/// is a flag rather than two variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeeLoop {
    /// `BeeFlyingSoundInstance` — `entity.bee.loop`, switches when angry.
    Flying,
    /// `BeeAggressiveSoundInstance` — `entity.bee.loop_aggressive`, switches
    /// when calm.
    Aggressive,
}

impl BeeLoop {
    /// `SoundEvents.BEE_LOOP` / `BEE_LOOP_AGGRESSIVE`.
    pub fn identifier(self) -> &'static str {
        match self {
            BeeLoop::Flying => "minecraft:entity.bee.loop",
            BeeLoop::Aggressive => "minecraft:entity.bee.loop_aggressive",
        }
    }

    fn other(self) -> BeeLoop {
        match self {
            BeeLoop::Flying => BeeLoop::Aggressive,
            BeeLoop::Aggressive => BeeLoop::Flying,
        }
    }

    /// `shouldSwitchSounds()`.
    fn should_switch(self, angry: bool) -> bool {
        match self {
            BeeLoop::Flying => angry,
            BeeLoop::Aggressive => !angry,
        }
    }
}

/// `BeeSoundInstance`'s state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeeRamp {
    pub bee: i32,
    pub loop_kind: BeeLoop,
    /// `hasSwitched` — set the tick the alternative is queued, and read in the
    /// *same* tick by the guard below, so the instance queues its replacement
    /// and stops itself without an intervening tick.
    pub has_switched: bool,
}

/// `ElytraOnPlayerSoundInstance`'s state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElytraRamp {
    pub player: i32,
    /// `time`, incremented at the very top of `tick()` — so the first tick
    /// sees `time == 1`, not 0.
    pub time: i32,
}

/// `MinecartSoundInstance`'s state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinecartRamp {
    pub minecart: i32,
    /// **The shadowed field.** `MinecartSoundInstance.java:16` declares
    /// `private float pitch = 0.0F;` over `AbstractSoundInstance`'s
    /// `protected float pitch = 1.0F;`, and `getPitch()` — declared in the
    /// superclass, so statically bound to the superclass field — never sees
    /// this one. `tick()` maintains it faithfully and nothing reads it.
    ///
    /// Reproduced rather than deleted so the ramp is visible and its deadness
    /// is a property a test can assert. See
    /// `a_minecarts_pitch_ramp_is_dead_code`.
    pub shadowed_pitch: f32,
}

/// `RidingEntitySoundInstance`'s constants, plus which subclass it is.
///
/// `RidingMinecartSoundInstance` overrides three protected hooks; each is a
/// field here rather than a variant, because nothing else about the class
/// differs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RidingRamp {
    pub player: i32,
    pub vehicle: i32,
    /// `underwaterSound` — which of the pair this instance is. Vanilla starts
    /// **both** minecart instances at once and lets each mute itself, so the
    /// crossfade between the dry and submerged loops is two permanently-live
    /// voices rather than a switch. A client that picked one would be silent
    /// for whichever half of the ride it guessed wrong.
    pub underwater_sound: bool,
    pub volume_min: f32,
    pub volume_max: f32,
    pub volume_amplifier: f32,
    /// `true` for `RidingMinecartSoundInstance`, which overrides
    /// `shouldNotPlayUnderwaterSound` to read the **player**'s submersion
    /// rather than the vehicle's, `getEntitySpeed` to be the horizontal
    /// distance rather than the full length, and `shoudlPlaySound` (vanilla's
    /// spelling) to require rails.
    pub is_minecart: bool,
}

/// `UnderwaterAmbientSoundInstances.UnderwaterAmbientSoundInstance`'s state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnderwaterRamp {
    pub player: i32,
    /// `fade`, 0 at construction. Rises by 1 while submerged, falls by **2**
    /// while not.
    pub fade: i32,
}

/// `BiomeAmbientSoundsHandler.LoopSoundInstance`'s state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeLoopRamp {
    /// `fadeDirection`, **0** at construction — so a fresh instance holds its
    /// volume at 0 forever until `fade_in`/`fade_out` is called.
    pub fade_direction: i32,
    pub fade: i32,
}

impl BiomeLoopRamp {
    /// `fadeIn()` — `fade = Math.max(0, fade); fadeDirection = 1;`.
    pub fn fade_in(&mut self) {
        self.fade = self.fade.max(0);
        self.fade_direction = 1;
    }

    /// `fadeOut()` — `fade = Math.min(fade, 40); fadeDirection = -1;`.
    ///
    /// Note the asymmetry with `fadeIn`: this one *caps* an over-large fade so
    /// a long-submerged loop starts fading immediately, where `fadeIn` floors
    /// a negative one so a nearly-dead loop restarts from silence rather than
    /// from below it.
    pub fn fade_out(&mut self) {
        self.fade = self.fade.min(UNDERWATER_FADE_DURATION);
        self.fade_direction = -1;
    }
}

/// One instance's `tick()` body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ramp {
    /// `EntityBoundSoundInstance` — follow, or stop when removed.
    EntityBound { entity: i32 },
    /// `BeeSoundInstance`.
    Bee(BeeRamp),
    /// `ElytraOnPlayerSoundInstance`.
    Elytra(ElytraRamp),
    /// `GuardianAttackSoundInstance`.
    Guardian {
        guardian: i32,
        /// 80, or 60 for an elder guardian.
        attack_duration: i32,
    },
    /// `MinecartSoundInstance`.
    Minecart(MinecartRamp),
    /// `RidingEntitySoundInstance` and `RidingMinecartSoundInstance`.
    Riding(RidingRamp),
    /// `SnifferSoundInstance`.
    Sniffer { sniffer: i32 },
    /// `UnderwaterAmbientSoundInstances.UnderwaterAmbientSoundInstance`.
    UnderwaterLoop(UnderwaterRamp),
    /// `UnderwaterAmbientSoundInstances.SubSound` — no ramp at all, only a
    /// stop condition.
    UnderwaterSub { player: i32 },
    /// `BiomeAmbientSoundsHandler.LoopSoundInstance`.
    BiomeLoop(BiomeLoopRamp),
    /// `DirectionalSoundInstance` — a fixed bearing from the camera, ten
    /// blocks out, repositioned every tick.
    ///
    /// Its one vanilla caller is the End flash
    /// (`ClientLevel.java:307-324`), and it was the **last** of the eleven
    /// variants to get a construction site — M149f built it, so every ramp
    /// here is now reachable from a running client.
    ///
    /// It is also the only sound this client **queues** rather than plays:
    /// `playDelayed(.., 30)`. That queue **ticks a tickable instance before
    /// playing it** (`SoundEngine.java:292-296`), so this ramp's
    /// `setPosition()` runs again at play time — the bearing is taken from the
    /// camera 30 ticks *after* the flash, not from where the listener stood
    /// when it fired. Capturing the position when it is queued is the natural
    /// implementation and is 1.5 seconds stale.
    Directional { x_angle: f32, y_angle: f32 },
}

/// What one `tick()` did.
///
/// Vanilla's `tick()` returns `void` and communicates through `stop()` (which
/// sets `stopped` *and* clears `looping`) and, for the bee alone, through a
/// call to `SoundManager.queueTickingSound`. Both are outputs, so both are
/// here rather than being side effects on a world handle.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RampOutcome {
    /// `AbstractTickableSoundInstance.stop()` ran.
    pub stopped: bool,
    /// `SoundManager.queueTickingSound(getAlternativeSoundInstance())` ran.
    ///
    /// Only the bee ever fills this, and when it does it also sets `stopped`
    /// in the same tick.
    pub queued: Option<(SoundInstance, Ramp)>,
}

impl Ramp {
    /// The ramp an ordinary (non-tickable) instance gets: `EntityBound` when
    /// it follows an entity, and none otherwise.
    ///
    /// **One derivation with two callers**, because `play` and the queue path
    /// both need it and `play_ramped(.., None)` is not the same thing —
    /// passing `None` for a `sound_entity` stops it following its entity, and
    /// the engine's own follow test calls `play` directly so it could not see
    /// that. M89's rule, applied before it could bite rather than after.
    pub fn for_instance(instance: &SoundInstance) -> Option<Ramp> {
        match instance.binding {
            crate::sound_instance::Binding::Entity(e) => Some(Ramp::EntityBound { entity: e }),
            crate::sound_instance::Binding::Fixed => None,
        }
    }

    /// The entity whose `isSilent()` gates this sound, if any —
    /// `SoundInstance.canPlaySound()`.
    ///
    /// **Six of the ten override it and four do not**, counted from the
    /// decompile rather than assumed: `EntityBoundSoundInstance`,
    /// `BeeSoundInstance`, `GuardianAttackSoundInstance`,
    /// `MinecartSoundInstance`, `RidingEntitySoundInstance` and
    /// `SnifferSoundInstance` each return `!x.isSilent()`; the elytra, both
    /// underwater instances, the biome loop and the directional sound take the
    /// interface default, which is a flat `true` (`SoundInstance.java:41`).
    ///
    /// So this is **not** [`Self::entity`]. Using the followed entity for both
    /// gives an elytra sound that a `/data`-silenced player would silence,
    /// which vanilla does not do — the class simply has no override. The
    /// battery found exactly that: `binding: Fixed` on the elytra instance
    /// survived, because with the two conflated the binding was carrying a
    /// gate that belongs to the ramp.
    ///
    /// `RidingEntitySoundInstance` gates on the **vehicle**, not the rider,
    /// which is also what it follows — so the two agree there and disagree
    /// only where a class declines the gate.
    pub fn silence_gated_entity(&self) -> Option<i32> {
        match self {
            Ramp::EntityBound { entity } => Some(*entity),
            Ramp::Bee(b) => Some(b.bee),
            Ramp::Guardian { guardian, .. } => Some(*guardian),
            Ramp::Minecart(m) => Some(m.minecart),
            Ramp::Riding(r) => Some(r.vehicle),
            Ramp::Sniffer { sniffer } => Some(*sniffer),
            // No `canPlaySound` override — the interface default is `true`.
            Ramp::Elytra(_)
            | Ramp::UnderwaterLoop(_)
            | Ramp::UnderwaterSub { .. }
            | Ramp::BiomeLoop(_)
            | Ramp::Directional { .. } => None,
        }
    }

    /// The entity this ramp follows, if any — what the engine needs to look up
    /// before it can tick.
    pub fn entity(&self) -> Option<i32> {
        match self {
            Ramp::EntityBound { entity } => Some(*entity),
            Ramp::Bee(b) => Some(b.bee),
            Ramp::Elytra(e) => Some(e.player),
            Ramp::Guardian { guardian, .. } => Some(*guardian),
            Ramp::Minecart(m) => Some(m.minecart),
            Ramp::Riding(r) => Some(r.vehicle),
            Ramp::Sniffer { sniffer } => Some(*sniffer),
            Ramp::UnderwaterLoop(u) => Some(u.player),
            Ramp::UnderwaterSub { player } => Some(*player),
            Ramp::BiomeLoop(_) | Ramp::Directional { .. } => None,
        }
    }

    /// One `tick()`.
    ///
    /// Writes volume, pitch and position into `inst` exactly where vanilla
    /// writes them into its own fields, and reports `stop()` rather than
    /// performing it — clearing `looping` is the engine's half, because it is
    /// the engine that knows whether a manual-loop requeue is pending.
    pub fn tick(&mut self, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
        match self {
            Ramp::EntityBound { entity } => tick_entity_bound(*entity, inst, w),
            Ramp::Bee(b) => tick_bee(b, inst, w),
            Ramp::Elytra(e) => tick_elytra(e, inst, w),
            Ramp::Guardian {
                guardian,
                attack_duration: _,
            } => tick_guardian(*guardian, inst, w),
            Ramp::Minecart(m) => tick_minecart(m, inst, w),
            Ramp::Riding(r) => tick_riding(r, inst, w),
            Ramp::Sniffer { sniffer } => tick_sniffer(*sniffer, inst, w),
            Ramp::UnderwaterLoop(u) => tick_underwater_loop(u, inst, w),
            Ramp::UnderwaterSub { player } => tick_underwater_sub(*player, inst, w),
            Ramp::BiomeLoop(b) => tick_biome_loop(b, inst),
            Ramp::Directional { x_angle, y_angle } => {
                tick_directional(*x_angle, *y_angle, inst, w);
                RampOutcome::default()
            }
        }
    }
}

/// The `this.x = (float)entity.getX()` triple every positioned body shares.
///
/// The `(float)` is not decorative — it is written into a `double` field, so
/// the narrowing is permanent and shows in both the attenuation gain and the
/// stereo pan for any entity far enough out that its coordinate needs more
/// than 24 bits of mantissa.
fn follow(inst: &mut SoundInstance, pos: (f64, f64, f64)) {
    inst.x = pos.0 as f32 as f64;
    inst.y = pos.1 as f32 as f64;
    inst.z = pos.2 as f32 as f64;
}

const STOPPED: RampOutcome = RampOutcome {
    stopped: true,
    queued: None,
};

/// `EntityBoundSoundInstance.tick()`.
fn tick_entity_bound(entity: i32, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    match w.position(entity) {
        None => STOPPED,
        Some(pos) => {
            follow(inst, pos);
            RampOutcome::default()
        }
    }
}

/// `BeeSoundInstance.tick()` — `BeeSoundInstance.java:27-50`.
fn tick_bee(b: &mut BeeRamp, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    let mut out = RampOutcome::default();

    // `if (shouldSwitchSounds && !this.isStopped())`. `isStopped()` can only
    // be true here if a previous tick stopped the instance, which the engine
    // reclaims, so the guard is defensive in vanilla too — transcribed rather
    // than reasoned away.
    if b.loop_kind.should_switch(w.angry(b.bee)) && !b.has_switched {
        let other = b.loop_kind.other();
        out.queued = Some((
            bee_instance(other, b.bee, w.position(b.bee).unwrap_or((0.0, 0.0, 0.0))),
            Ramp::Bee(BeeRamp {
                bee: b.bee,
                loop_kind: other,
                has_switched: false,
            }),
        ));
        b.has_switched = true;
    }

    let Some(pos) = w.position(b.bee) else {
        out.stopped = true;
        return out;
    };
    if b.has_switched {
        // Same tick: queued above, stopped here. The `else` branch is reached
        // through `hasSwitched`, not through a second tick.
        out.stopped = true;
        return out;
    }

    follow(inst, pos);
    let speed = w.horizontal_speed(b.bee) as f32;
    if speed >= 0.01 {
        let (min_pitch, max_pitch) = bee_pitch_range(w.baby(b.bee));
        // The factor is clamped to the PITCH range, not to 0..1 — see the
        // module doc. This is vanilla's line, verbatim.
        inst.pitch = mth_lerp(mth_clamp(speed, min_pitch, max_pitch), min_pitch, max_pitch);
        // …and this factor saturates at 0.5, so 1.2 is a ceiling of 0.6.
        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 0.5), 0.0, 1.2);
    } else {
        inst.pitch = 0.0;
        inst.volume = 0.0;
    }
    out
}

/// `getMinPitch()` / `getMaxPitch()` — `BeeSoundInstance.java:52-58`.
pub fn bee_pitch_range(baby: bool) -> (f32, f32) {
    if baby {
        (1.1, 1.5)
    } else {
        (0.7, 1.1)
    }
}

/// `new BeeFlyingSoundInstance(bee)` / `new BeeAggressiveSoundInstance(bee)` —
/// the constructor in `BeeSoundInstance.java:16-25`.
///
/// `volume = 0.0F` and `canStartSilent()` is `true`, which is the pair that
/// lets a bee's loop start at all: `SoundEngine.play` drops a zero-volume
/// instance unless it says it can start silent.
pub fn bee_instance(kind: BeeLoop, bee: i32, pos: (f64, f64, f64)) -> SoundInstance {
    SoundInstance {
        volume: 0.0,
        looping: true,
        delay: 0,
        can_start_silent: true,
        x: pos.0 as f32 as f64,
        y: pos.1 as f32 as f64,
        z: pos.2 as f32 as f64,
        binding: crate::sound_instance::Binding::Entity(bee),
        ..SoundInstance::bare(kind.identifier(), SoundSource::Neutral)
    }
}

/// `ElytraOnPlayerSoundInstance.tick()` — `ElytraOnPlayerSoundInstance.java:21-50`.
fn tick_elytra(e: &mut ElytraRamp, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    // `this.time++` is the first statement, so the first tick sees 1.
    e.time += 1;

    let Some(pos) = w.position(e.player) else {
        return STOPPED;
    };
    // `time <= 20 || isFallFlying()` — the sound survives its first twenty
    // ticks whether or not the player is still flying, which is what lets it
    // exist during the launch frames.
    if !(e.time <= ELYTRA_DELAY || w.fall_flying(e.player)) {
        return STOPPED;
    }

    follow(inst, pos);
    // `lengthSqr()`, NOT `length()` — a squared speed, then quartered.
    let speed = w.speed_sqr(e.player) as f32;
    inst.volume = if speed as f64 >= 1.0e-7 {
        mth_clamp(speed / 4.0, 0.0, 1.0)
    } else {
        0.0
    };

    if e.time < ELYTRA_DELAY {
        inst.volume = 0.0;
    } else if e.time < 2 * ELYTRA_DELAY {
        // At `time == 20` the factor is exactly 0, so the sound is silent for
        // ticks 1..=20 inclusive and fades over 21..=39.
        inst.volume *= (e.time - ELYTRA_DELAY) as f32 / ELYTRA_DELAY as f32;
    }

    // The pitch reads the POST-fade volume, so it stays at 1 for the whole
    // fade-in however fast the player is going.
    inst.pitch = if inst.volume > 0.8 {
        1.0 + (inst.volume - 0.8)
    } else {
        1.0
    };
    RampOutcome::default()
}

/// `GuardianAttackSoundInstance.tick()` — `GuardianAttackSoundInstance.java:27-39`.
fn tick_guardian(guardian: i32, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    let Some(pos) = w.position(guardian) else {
        return STOPPED;
    };
    // `guardian.getTarget() == null`. Always true on a real client — see
    // `RampWorld::has_ai_target`.
    if w.has_ai_target(guardian) {
        return STOPPED;
    }
    follow(inst, pos);
    let scale = w.attack_animation_scale(guardian);
    // `0.0F + 1.0F * scale * scale` — the volume is the scale SQUARED while
    // the pitch is linear in it, so the beam ramps in loudness far more
    // slowly than it does in pitch.
    inst.volume = 0.0 + 1.0 * scale * scale;
    inst.pitch = 0.7 + 0.5 * scale;
    RampOutcome::default()
}

/// `MinecartSoundInstance.tick()` — `MinecartSoundInstance.java:39-57`.
fn tick_minecart(m: &mut MinecartRamp, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    let Some(pos) = w.position(m.minecart) else {
        return STOPPED;
    };
    follow(inst, pos);
    let speed = w.horizontal_speed(m.minecart) as f32;
    // `!isOnRails() && behavior instanceof NewMinecartBehavior` — a cart on
    // the old behaviour is never "off rail" however far from a rail it is.
    let off_rail = !w.on_rails(m.minecart) && w.new_minecart_behavior(m.minecart);
    if speed >= 0.01 && w.runs_normally() && !off_rail {
        // Writes the SHADOW. Nothing reads it; see the module doc.
        m.shadowed_pitch = mth_clamp(m.shadowed_pitch + 0.0025, 0.0, 1.0);
        // Factor saturates at 0.5, so the ceiling is 0.35 and not 0.7.
        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 0.5), 0.0, 0.7);
    } else {
        m.shadowed_pitch = 0.0;
        inst.volume = 0.0;
    }
    RampOutcome::default()
}

/// `RidingEntitySoundInstance.tick()` — `RidingEntitySoundInstance.java:62-76`.
fn tick_riding(r: &RidingRamp, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    if w.position(r.vehicle).is_none() || w.vehicle_of(r.player) != Some(r.vehicle) {
        return STOPPED;
    }

    // `shouldNotPlayUnderwaterSound()` — the base class asks the **vehicle**,
    // `RidingMinecartSoundInstance` overrides it to ask the **player**. The
    // two disagree exactly when a cart is submerged and its rider is not, or
    // the reverse, which is most of the time a cart enters water.
    let submerged = if r.is_minecart {
        w.underwater(r.player)
    } else {
        w.underwater(r.vehicle)
    };
    if r.underwater_sound != submerged {
        inst.volume = r.volume_min;
        return RampOutcome::default();
    }

    // `getEntitySpeed()` — full length in the base class, horizontal only for
    // a minecart, so a cart falling makes no more noise than one at rest.
    let speed = if r.is_minecart {
        w.horizontal_speed(r.vehicle) as f32
    } else {
        w.speed(r.vehicle) as f32
    };
    // `shoudlPlaySound()` (vanilla's spelling) — always true in the base
    // class, rails-or-old-behaviour for a minecart.
    let should_play = !r.is_minecart
        || w.on_rails(r.vehicle)
        || !w.new_minecart_behavior(r.vehicle);

    inst.volume = if speed >= 0.01 && should_play {
        // `clampedLerp`, so the FACTOR is clamped to 0..1 — a different
        // function from the bee's and the minecart's `lerp(clamp(..))`.
        r.volume_amplifier * mth_clamped_lerp(speed, r.volume_min, r.volume_max)
    } else {
        r.volume_min
    };
    RampOutcome::default()
}

/// `SnifferSoundInstance.tick()` — `SnifferSoundInstance.java:25-36`.
fn tick_sniffer(sniffer: i32, inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    let Some(pos) = w.position(sniffer) else {
        return STOPPED;
    };
    if w.has_ai_target(sniffer) || !w.sniffer_digging(sniffer) {
        return STOPPED;
    }
    follow(inst, pos);
    // Constant, and assigned every tick rather than once — which matters
    // because nothing else would restore them if some other path moved them.
    inst.volume = 1.0;
    inst.pitch = 1.0;
    RampOutcome::default()
}

/// `UnderwaterAmbientSoundInstance.tick()` —
/// `UnderwaterAmbientSoundInstances.java:43-57`.
fn tick_underwater_loop(
    u: &mut UnderwaterRamp,
    inst: &mut SoundInstance,
    w: &dyn RampWorld,
) -> RampOutcome {
    // The guard reads `fade` BEFORE the update, so a fade that goes negative
    // this tick still writes its volume and stops on the next one.
    if w.position(u.player).is_none() || u.fade < 0 {
        return STOPPED;
    }
    if w.underwater(u.player) {
        u.fade += 1;
    } else {
        // Twice as fast down as up. Not symmetric, and not capped below.
        u.fade -= 2;
    }
    u.fade = u.fade.min(UNDERWATER_FADE_DURATION);
    inst.volume = 0.0_f32.max((u.fade as f32 / UNDERWATER_FADE_DURATION as f32).min(1.0));
    RampOutcome::default()
}

/// `UnderwaterAmbientSoundInstances.SubSound.tick()` — the whole body.
fn tick_underwater_sub(player: i32, _inst: &mut SoundInstance, w: &dyn RampWorld) -> RampOutcome {
    if w.position(player).is_none() || !w.underwater(player) {
        return STOPPED;
    }
    RampOutcome::default()
}

/// `BiomeAmbientSoundsHandler.LoopSoundInstance.tick()` —
/// `BiomeAmbientSoundsHandler.java:122-130`.
fn tick_biome_loop(b: &mut BiomeLoopRamp, inst: &mut SoundInstance) -> RampOutcome {
    // **No `else`.** Every other body here stops *instead of* ramping; this
    // one stops and then ramps anyway, so a dying loop's last volume is still
    // written. Transcribed rather than tidied.
    let stopped = b.fade < 0;
    b.fade += b.fade_direction;
    inst.volume = mth_clamp(
        b.fade as f32 / UNDERWATER_FADE_DURATION as f32,
        0.0,
        1.0,
    );
    RampOutcome {
        stopped,
        queued: None,
    }
}

/// `DirectionalSoundInstance.setPosition()` —
/// `DirectionalSoundInstance.java:24-30`.
///
/// Ten blocks from the camera along a fixed bearing, re-read every tick, with
/// `Attenuation.NONE` reasserted each time. The bearing is `Vec3
/// .directionFromRotation`, which samples `Mth`'s 65,536-entry sine table
/// rather than platform trig — reused from `rewo_world::lightmap` rather than
/// re-derived (M12 pinned it there).
fn tick_directional(x_angle: f32, y_angle: f32, inst: &mut SoundInstance, w: &dyn RampWorld) {
    let (dx, dy, dz) = direction_from_rotation(x_angle, y_angle);
    let cam = w.camera_position();
    inst.x = cam.0 + dx * 10.0;
    inst.y = cam.1 + dy * 10.0;
    inst.z = cam.2 + dz * 10.0;
    inst.attenuation = Attenuation::None;
}

/// `Vec3.directionFromRotation(float rotX, float rotY)` — `Vec3.java:267-273`.
///
/// Note `xCos` is **negated** and both angles are negated before the degree
/// conversion, and the yaw term carries an extra `- PI`. Reproducing this from
/// the usual "yaw/pitch to vector" identity gives a direction reflected in two
/// axes.
pub fn direction_from_rotation(rot_x: f32, rot_y: f32) -> (f64, f64, f64) {
    use rewo_world::lightmap::{mth_cos, mth_sin};
    const DEG: f32 = std::f32::consts::PI / 180.0;
    let y_cos = mth_cos(-rot_y * DEG - std::f32::consts::PI);
    let y_sin = mth_sin(-rot_y * DEG - std::f32::consts::PI);
    let x_cos = -mth_cos(-rot_x * DEG);
    let x_sin = mth_sin(-rot_x * DEG);
    (
        (y_sin * x_cos) as f64,
        x_sin as f64,
        (y_cos * x_cos) as f64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A [`RampWorld`] whose every answer is a field, so a test states its
    /// inputs rather than reaching through a live session.
    #[derive(Clone, Default)]
    struct World {
        positions: HashMap<i32, (f64, f64, f64)>,
        horizontal_speed: HashMap<i32, f64>,
        speed: HashMap<i32, f64>,
        speed_sqr: HashMap<i32, f64>,
        baby: Vec<i32>,
        angry: Vec<i32>,
        underwater: Vec<i32>,
        ai_target: Vec<i32>,
        attack_scale: HashMap<i32, f32>,
        digging: Vec<i32>,
        on_rails: Vec<i32>,
        new_behavior: Vec<i32>,
        runs_normally: bool,
        fall_flying: Vec<i32>,
        vehicles: HashMap<i32, i32>,
        camera: (f64, f64, f64),
    }

    impl World {
        fn at(entity: i32, pos: (f64, f64, f64)) -> World {
            World {
                positions: HashMap::from([(entity, pos)]),
                runs_normally: true,
                ..World::default()
            }
        }
    }

    impl RampWorld for World {
        fn position(&self, e: i32) -> Option<(f64, f64, f64)> {
            self.positions.get(&e).copied()
        }
        fn horizontal_speed(&self, e: i32) -> f64 {
            self.horizontal_speed.get(&e).copied().unwrap_or(0.0)
        }
        fn speed(&self, e: i32) -> f64 {
            self.speed.get(&e).copied().unwrap_or(0.0)
        }
        fn speed_sqr(&self, e: i32) -> f64 {
            self.speed_sqr.get(&e).copied().unwrap_or(0.0)
        }
        fn baby(&self, e: i32) -> bool {
            self.baby.contains(&e)
        }
        fn angry(&self, e: i32) -> bool {
            self.angry.contains(&e)
        }
        fn underwater(&self, e: i32) -> bool {
            self.underwater.contains(&e)
        }
        fn has_ai_target(&self, e: i32) -> bool {
            self.ai_target.contains(&e)
        }
        fn attack_animation_scale(&self, e: i32) -> f32 {
            self.attack_scale.get(&e).copied().unwrap_or(0.0)
        }
        fn sniffer_digging(&self, e: i32) -> bool {
            self.digging.contains(&e)
        }
        fn on_rails(&self, e: i32) -> bool {
            self.on_rails.contains(&e)
        }
        fn new_minecart_behavior(&self, e: i32) -> bool {
            self.new_behavior.contains(&e)
        }
        fn runs_normally(&self) -> bool {
            self.runs_normally
        }
        fn fall_flying(&self, e: i32) -> bool {
            self.fall_flying.contains(&e)
        }
        fn vehicle_of(&self, e: i32) -> Option<i32> {
            self.vehicles.get(&e).copied()
        }
        fn camera_position(&self) -> (f64, f64, f64) {
            self.camera
        }
    }

    fn inst() -> SoundInstance {
        SoundInstance::bare("minecraft:test", SoundSource::Neutral)
    }

    // ---- Mth ------------------------------------------------------------

    /// **`clampedLerp` and `lerp(clamp(..))` are different functions**, and
    /// this module uses both. They agree only where the factor is inside
    /// `0..1` *and* inside `min..max`.
    #[test]
    fn clamped_lerp_is_not_lerp_of_a_clamped_factor() {
        // A factor of 0.5 with an output band of 0.7..1.1: `clampedLerp`
        // interpolates (0.9), the other form clamps the factor UP to 0.7 and
        // interpolates with that (0.98).
        assert!((mth_clamped_lerp(0.5, 0.7, 1.1) - 0.9).abs() < 1e-6);
        assert!((mth_lerp(mth_clamp(0.5, 0.7, 1.1), 0.7, 1.1) - 0.98).abs() < 1e-6);
        // …and at a factor above 1 they diverge the other way.
        assert!((mth_clamped_lerp(2.0, 0.0, 0.7) - 0.7).abs() < 1e-6);
        assert!((mth_lerp(mth_clamp(2.0, 0.0, 0.5), 0.0, 0.7) - 0.35).abs() < 1e-6);
        // Zero is the one place they must agree.
        assert_eq!(mth_clamped_lerp(0.0, 0.25, 0.9), mth_lerp(0.0, 0.25, 0.9));
    }

    /// `isAngry()` is two strict tests, and a bee's default is **-1**.
    ///
    /// **The first conjunct needs a negative game time to witness at all**, and
    /// the obvious fixture cannot supply one. For any `gameTime >= 0`,
    /// `endTime - gameTime > 0` already implies `endTime > 0`, so deleting the
    /// first test changes no answer a running server can produce — the battery
    /// found exactly that, and the first version of this test had `(-1, 0)` and
    /// `(0, 0)`, both of which the mutant also answers "calm".
    ///
    /// It is **not** an equivalent mutant, though, because the input region is
    /// representable: `set_time` carries a signed i64 and Rewo does not reject
    /// a negative one. So the discriminating pair is pinned rather than the
    /// guard being declared dead.
    #[test]
    fn anger_is_a_deadline_and_minus_one_is_calm() {
        assert!(!is_angry(-1, 0), "the DATA_ANGER_END_TIME default");
        assert!(!is_angry(0, 0), "endTime > 0 is strict, so 0 is calm");
        assert!(is_angry(100, 99));
        assert!(!is_angry(100, 100), "the second test is strict too");
        assert!(!is_angry(100, 101));
        // A large game time cannot revive an expired deadline.
        assert!(!is_angry(100, 1_000_000));

        // The one region where the two conjuncts disagree: a never-angered bee
        // (endTime -1) at a negative game time. Dropping `endTime > 0` makes
        // `-1 - (-5) = 4 > 0` and reports it furious.
        assert!(!is_angry(-1, -5), "the first conjunct is load-bearing here");
        assert!(!is_angry(0, -5));
    }

    // ---- the minecart's dead pitch ramp ----------------------------------

    /// **The headline.** `MinecartSoundInstance.java:16` shadows
    /// `AbstractSoundInstance.pitch`, and `getPitch()` — declared in the
    /// superclass — reads the superclass field. So the ramp runs, and the
    /// instance's pitch never moves.
    ///
    /// A transcription that reads the class in isolation gives a twenty-second
    /// pitch sweep at the start of every ride, which is the most audible thing
    /// you could get wrong here.
    #[test]
    fn a_minecarts_pitch_ramp_is_dead_code() {
        let mut w = World::at(1, (0.0, 64.0, 0.0));
        w.horizontal_speed.insert(1, 0.3);
        w.on_rails.push(1);
        let mut r = Ramp::Minecart(MinecartRamp {
            minecart: 1,
            shadowed_pitch: 0.0,
        });
        let mut i = inst();
        let pitch_before = i.pitch;
        for _ in 0..400 {
            r.tick(&mut i, &w);
        }
        assert_eq!(
            i.pitch, pitch_before,
            "the instance's pitch must not move; only the shadow does"
        );
        assert_eq!(i.pitch, 1.0, "AbstractSoundInstance's field default");

        // …and the shadow really did ramp, so this is a shadow rather than a
        // missing implementation. 400 ticks at 0.0025 saturates the clamp.
        let Ramp::Minecart(m) = r else { unreachable!() };
        assert!(
            (m.shadowed_pitch - 1.0).abs() < 1e-6,
            "the ramp ran: {}",
            m.shadowed_pitch
        );
    }

    /// The shadow is reset to 0 whenever the cart stalls, so a cart that stops
    /// and starts restarts its (dead) ramp from the bottom.
    #[test]
    fn the_minecart_shadow_resets_when_the_cart_stalls() {
        let mut w = World::at(1, (0.0, 64.0, 0.0));
        w.horizontal_speed.insert(1, 0.3);
        w.on_rails.push(1);
        let mut r = Ramp::Minecart(MinecartRamp {
            minecart: 1,
            shadowed_pitch: 0.0,
        });
        let mut i = inst();
        for _ in 0..40 {
            r.tick(&mut i, &w);
        }
        let Ramp::Minecart(m) = r else { unreachable!() };
        assert!(m.shadowed_pitch > 0.0);

        w.horizontal_speed.insert(1, 0.0);
        r.tick(&mut i, &w);
        let Ramp::Minecart(m) = r else { unreachable!() };
        assert_eq!(m.shadowed_pitch, 0.0);
        assert_eq!(i.volume, 0.0);
    }

    /// **The minecart's volume ceiling is 0.35, not the `VOLUME_MAX = 0.7F`
    /// the class declares** — the lerp factor saturates at 0.5.
    #[test]
    fn the_minecart_volume_ceiling_is_half_its_named_maximum() {
        let mut w = World::at(1, (0.0, 64.0, 0.0));
        w.on_rails.push(1);
        let mut r = Ramp::Minecart(MinecartRamp {
            minecart: 1,
            shadowed_pitch: 0.0,
        });
        let mut i = inst();
        for &(speed, expected) in &[(0.5_f64, 0.35_f32), (5.0, 0.35), (0.25, 0.175)] {
            w.horizontal_speed.insert(1, speed);
            r.tick(&mut i, &w);
            assert!(
                (i.volume - expected).abs() < 1e-6,
                "speed {speed} gave {}, expected {expected}",
                i.volume
            );
        }
    }

    /// A cart off the new-behaviour rails is silent even at speed — and one on
    /// the OLD behaviour is not, because the predicate is a conjunction.
    #[test]
    fn only_a_new_behaviour_cart_off_rails_goes_silent() {
        let mut w = World::at(1, (0.0, 64.0, 0.0));
        w.horizontal_speed.insert(1, 0.5);
        let mut r = Ramp::Minecart(MinecartRamp {
            minecart: 1,
            shadowed_pitch: 0.0,
        });
        let mut i = inst();

        // New behaviour, off rails → silent.
        w.new_behavior.push(1);
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 0.0);

        // Old behaviour, off rails → audible. Dropping the second conjunct
        // silences every cart on the old behaviour.
        w.new_behavior.clear();
        r.tick(&mut i, &w);
        assert!((i.volume - 0.35).abs() < 1e-6, "{}", i.volume);

        // New behaviour but on rails → audible.
        w.new_behavior.push(1);
        w.on_rails.push(1);
        r.tick(&mut i, &w);
        assert!((i.volume - 0.35).abs() < 1e-6, "{}", i.volume);
    }

    /// A frozen tick rate silences the cart, which is the third term of the
    /// same `if` and independent of the other two.
    #[test]
    fn a_frozen_tick_rate_silences_the_minecart() {
        let mut w = World::at(1, (0.0, 64.0, 0.0));
        w.horizontal_speed.insert(1, 0.5);
        w.on_rails.push(1);
        w.runs_normally = false;
        let mut r = Ramp::Minecart(MinecartRamp {
            minecart: 1,
            shadowed_pitch: 0.0,
        });
        let mut i = inst();
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 0.0);
    }

    // ---- the bee ---------------------------------------------------------

    /// **An adult bee's pitch is a constant 0.98**, because the lerp factor is
    /// clamped to the pitch range instead of to `0..1`. The `0.7..1.1` band
    /// the getters name is never entered.
    #[test]
    fn a_bees_pitch_is_pinned_by_the_clamped_lerp_factor() {
        let mut w = World::at(9, (0.0, 64.0, 0.0));
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let mut i = inst();
        // Every speed a bee can reach is below the 0.7 lower clamp, so the
        // factor is 0.7 and the pitch is 0.7 + 0.7*0.4 = 0.98 throughout.
        for &speed in &[0.01_f64, 0.05, 0.2, 0.6] {
            w.horizontal_speed.insert(9, speed);
            r.tick(&mut i, &w);
            assert!(
                (i.pitch - 0.98).abs() < 1e-6,
                "speed {speed} gave pitch {}",
                i.pitch
            );
        }
        // A factor inside the band does move it, which is what proves the
        // clamp bound rather than a hardcoded constant is doing the work.
        w.horizontal_speed.insert(9, 0.9);
        r.tick(&mut i, &w);
        assert!((i.pitch - (0.7 + 0.9 * 0.4)).abs() < 1e-6, "{}", i.pitch);
    }

    /// A baby bee's band is `1.1..1.5`, so its pinned pitch is 1.54 — above
    /// the adult's and above 1.0, which is the audible difference.
    #[test]
    fn a_baby_bees_pitch_is_pinned_higher() {
        let mut w = World::at(9, (0.0, 64.0, 0.0));
        w.baby.push(9);
        w.horizontal_speed.insert(9, 0.2);
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let mut i = inst();
        r.tick(&mut i, &w);
        assert!((i.pitch - 1.54).abs() < 1e-6, "{}", i.pitch);
        assert_eq!(bee_pitch_range(true), (1.1, 1.5));
        assert_eq!(bee_pitch_range(false), (0.7, 1.1));
    }

    /// **A bee's volume ceiling is 0.6, not the declared `VOLUME_MAX = 1.2F`.**
    #[test]
    fn a_bees_volume_ceiling_is_half_its_named_maximum() {
        let mut w = World::at(9, (0.0, 64.0, 0.0));
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let mut i = inst();
        for &(speed, expected) in &[(0.5_f64, 0.6_f32), (2.0, 0.6), (0.25, 0.3)] {
            w.horizontal_speed.insert(9, speed);
            r.tick(&mut i, &w);
            assert!(
                (i.volume - expected).abs() < 1e-6,
                "speed {speed} gave {}",
                i.volume
            );
        }
    }

    /// Below 0.01 both channels go to **zero**, not to their minima — and a
    /// pitch of 0 is later clamped to 0.5 by `calculatePitch`, not left at 0.
    #[test]
    fn a_slow_bee_is_silent_and_pitch_zero() {
        let mut w = World::at(9, (0.0, 64.0, 0.0));
        w.horizontal_speed.insert(9, 0.009);
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let mut i = inst();
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 0.0);
        assert_eq!(i.pitch, 0.0);
        assert_eq!(
            crate::sound_instance::calculate_pitch(i.pitch),
            0.5,
            "the engine's clamp is what a device actually sees"
        );
    }

    /// **The switch queues its replacement and stops in the SAME tick.**
    /// Vanilla sets `hasSwitched` in the first `if` and the second `if` reads
    /// it immediately, so there is no tick in which both loops are live.
    #[test]
    fn an_angered_bee_queues_the_aggressive_loop_and_stops_at_once() {
        let mut w = World::at(9, (1.5, 64.0, -2.5));
        w.horizontal_speed.insert(9, 0.3);
        w.angry.push(9);
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let mut i = inst();
        let out = r.tick(&mut i, &w);

        assert!(out.stopped, "same tick, not the next one");
        let (queued_inst, queued_ramp) = out.queued.expect("the alternative was queued");
        assert_eq!(queued_inst.identifier, "minecraft:entity.bee.loop_aggressive");
        assert_eq!(queued_inst.volume, 0.0, "it starts silent…");
        assert!(queued_inst.can_start_silent, "…and is allowed to");
        assert!(queued_inst.looping);
        assert_eq!(queued_ramp.entity(), Some(9));
        let Ramp::Bee(b) = queued_ramp else {
            panic!("the replacement is a bee ramp")
        };
        assert_eq!(b.loop_kind, BeeLoop::Aggressive);
        assert!(!b.has_switched, "the replacement starts fresh");

        // The ramp did NOT run on the switching tick: the else-branch is
        // reached through `hasSwitched`, so volume stays where it was.
        assert_eq!(i.volume, 1.0, "bare()'s default, untouched");
    }

    /// The aggressive loop switches back on the opposite condition, so the two
    /// are not "one switches and the other is terminal".
    #[test]
    fn a_calmed_bee_switches_back_to_the_flying_loop() {
        let w = World::at(9, (0.0, 64.0, 0.0)); // not angry
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Aggressive,
            has_switched: false,
        });
        let mut i = inst();
        let out = r.tick(&mut i, &w);
        assert!(out.stopped);
        let (queued, _) = out.queued.expect("queued");
        assert_eq!(queued.identifier, "minecraft:entity.bee.loop");

        // …and an aggressive loop on an ANGRY bee does not switch.
        let mut w2 = World::at(9, (0.0, 64.0, 0.0));
        w2.angry.push(9);
        let mut r2 = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Aggressive,
            has_switched: false,
        });
        let out2 = r2.tick(&mut inst(), &w2);
        assert!(!out2.stopped);
        assert!(out2.queued.is_none());
    }

    /// A removed bee stops without queuing anything.
    #[test]
    fn a_removed_bee_stops_and_queues_nothing() {
        let w = World::default();
        let mut r = Ramp::Bee(BeeRamp {
            bee: 9,
            loop_kind: BeeLoop::Flying,
            has_switched: false,
        });
        let out = r.tick(&mut inst(), &w);
        assert!(out.stopped);
        assert!(out.queued.is_none());
    }

    // ---- the elytra ------------------------------------------------------

    /// **Silent for ticks 1..=20, then a linear fade over 21..=39.** The
    /// factor is `(time - 20) / 20`, which is exactly 0 at tick 20 — so the
    /// silent window is twenty-one ticks of `tick()` calls, not twenty.
    #[test]
    fn the_elytra_fades_in_over_its_second_twenty_ticks() {
        let mut w = World::at(3, (0.0, 100.0, 0.0));
        w.fall_flying.push(3);
        w.speed_sqr.insert(3, 4.0); // /4 → an un-faded volume of exactly 1.0
        let mut r = Ramp::Elytra(ElytraRamp { player: 3, time: 0 });
        let mut i = inst();

        for tick in 1..=20 {
            r.tick(&mut i, &w);
            assert_eq!(i.volume, 0.0, "tick {tick} must be silent");
        }
        // tick 21 → (21-20)/20 = 0.05
        r.tick(&mut i, &w);
        assert!((i.volume - 0.05).abs() < 1e-6, "{}", i.volume);
        for _ in 22..40 {
            r.tick(&mut i, &w);
        }
        // tick 39 → 19/20
        assert!((i.volume - 0.95).abs() < 1e-6, "{}", i.volume);
        r.tick(&mut i, &w); // tick 40: the fade is over
        assert_eq!(i.volume, 1.0);
    }

    /// **The speed is `lengthSqr`, quartered** — not a rooted speed. A client
    /// that rooted first reaches full volume at a quarter of the real speed.
    #[test]
    fn the_elytra_volume_is_a_quartered_squared_speed() {
        let mut w = World::at(3, (0.0, 100.0, 0.0));
        w.fall_flying.push(3);
        let mut r = Ramp::Elytra(ElytraRamp {
            player: 3,
            time: 100, // past the fade
        });
        let mut i = inst();
        for &(sqr, expected) in &[(1.0_f64, 0.25_f32), (2.0, 0.5), (4.0, 1.0), (400.0, 1.0)] {
            w.speed_sqr.insert(3, sqr);
            r.tick(&mut i, &w);
            assert!(
                (i.volume - expected).abs() < 1e-6,
                "lengthSqr {sqr} gave {}",
                i.volume
            );
        }
        // Below 1e-7 the branch assigns a flat zero rather than a tiny value.
        w.speed_sqr.insert(3, 1.0e-8);
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 0.0);
    }

    /// **The pitch reads the post-fade volume**, so it stays at 1 through the
    /// whole fade-in however fast the player is going.
    #[test]
    fn the_elytra_pitch_follows_the_faded_volume_not_the_raw_speed() {
        let mut w = World::at(3, (0.0, 100.0, 0.0));
        w.fall_flying.push(3);
        w.speed_sqr.insert(3, 400.0); // saturates the raw volume at 1.0
        let mut r = Ramp::Elytra(ElytraRamp { player: 3, time: 25 });
        let mut i = inst();
        r.tick(&mut i, &w); // time 26 → factor 6/20 = 0.3
        assert!((i.volume - 0.3).abs() < 1e-6, "{}", i.volume);
        assert_eq!(i.pitch, 1.0, "0.3 is below the 0.8 threshold");

        let mut r = Ramp::Elytra(ElytraRamp { player: 3, time: 100 });
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 1.0);
        assert!((i.pitch - 1.2).abs() < 1e-6, "1.0 + (1.0 - 0.8)");
    }

    /// **The first twenty ticks survive whether or not the player is
    /// flying** — `time <= 20 || isFallFlying()`.
    #[test]
    fn the_elytra_survives_its_first_twenty_ticks_without_flying() {
        let w = World::at(3, (0.0, 100.0, 0.0)); // NOT fall-flying
        let mut r = Ramp::Elytra(ElytraRamp { player: 3, time: 0 });
        let mut i = inst();
        for tick in 1..=20 {
            assert!(!r.tick(&mut i, &w).stopped, "tick {tick} must survive");
        }
        assert!(r.tick(&mut i, &w).stopped, "tick 21 must not");
    }

    // ---- the guardian ----------------------------------------------------

    /// **Volume is the attack scale SQUARED; pitch is linear in it.**
    #[test]
    fn the_guardian_volume_is_squared_and_its_pitch_is_not() {
        let mut w = World::at(5, (0.0, 40.0, 0.0));
        let mut r = Ramp::Guardian {
            guardian: 5,
            attack_duration: GUARDIAN_ATTACK_DURATION,
        };
        let mut i = inst();
        for &scale in &[0.0_f32, 0.25, 0.5, 1.0] {
            w.attack_scale.insert(5, scale);
            r.tick(&mut i, &w);
            assert!((i.volume - scale * scale).abs() < 1e-6, "{}", i.volume);
            assert!((i.pitch - (0.7 + 0.5 * scale)).abs() < 1e-6, "{}", i.pitch);
        }
        // Halfway through the wind-up the beam is a QUARTER as loud, which a
        // linear volume would render as half.
        w.attack_scale.insert(5, 0.5);
        r.tick(&mut i, &w);
        assert!((i.volume - 0.25).abs() < 1e-6);
    }

    /// The scale itself differs per species: the same counter is divided by
    /// **80** for a guardian and **60** for an elder.
    #[test]
    fn the_elder_guardians_ramp_is_a_third_faster() {
        assert_eq!(GUARDIAN_ATTACK_DURATION, 80);
        assert_eq!(ELDER_GUARDIAN_ATTACK_DURATION, 60);
        let at = 30;
        assert!((attack_animation_scale(at, GUARDIAN_ATTACK_DURATION) - 0.375).abs() < 1e-6);
        assert!((attack_animation_scale(at, ELDER_GUARDIAN_ATTACK_DURATION) - 0.5).abs() < 1e-6);
    }

    /// **`getTarget()` is never synced, so the guard reduces to
    /// `!isRemoved()`** — but the question is still asked, and answering it
    /// `true` really does stop the sound.
    #[test]
    fn an_ai_target_stops_the_guardian_though_a_client_never_sees_one() {
        let mut w = World::at(5, (0.0, 40.0, 0.0));
        w.attack_scale.insert(5, 1.0);
        let mut r = Ramp::Guardian {
            guardian: 5,
            attack_duration: GUARDIAN_ATTACK_DURATION,
        };
        assert!(!r.tick(&mut inst(), &w).stopped);
        w.ai_target.push(5);
        assert!(r.tick(&mut inst(), &w).stopped);
    }

    // ---- riding ----------------------------------------------------------

    fn riding(is_minecart: bool, underwater_sound: bool) -> Ramp {
        Ramp::Riding(RidingRamp {
            player: 1,
            vehicle: 2,
            underwater_sound,
            volume_min: 0.0,
            volume_max: 0.75,
            volume_amplifier: 1.0,
            is_minecart,
        })
    }

    /// **`clampedLerp`, so the factor is clamped to `0..1`** — the ceiling is
    /// the declared `volumeMax` and is reached at speed 1.0, unlike the bee's
    /// and the minecart's.
    #[test]
    fn the_riding_ramp_reaches_its_declared_maximum() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        w.on_rails.push(2);
        let mut r = riding(true, false);
        let mut i = inst();
        for &(speed, expected) in &[(0.5_f64, 0.375_f32), (1.0, 0.75), (9.0, 0.75)] {
            w.horizontal_speed.insert(2, speed);
            r.tick(&mut i, &w);
            assert!(
                (i.volume - expected).abs() < 1e-6,
                "speed {speed} gave {}",
                i.volume
            );
        }
    }

    /// **The amplifier multiplies the interpolated value**, so a happy ghast's
    /// 5.0 pushes the instance volume well above 1 — which extends its range
    /// rather than its loudness, exactly as a jukebox's 4.0 does.
    #[test]
    fn the_amplifier_multiplies_after_the_interpolation() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        w.speed.insert(2, 1.0);
        let mut r = Ramp::Riding(RidingRamp {
            player: 1,
            vehicle: 2,
            underwater_sound: false,
            volume_min: 0.0,
            volume_max: 1.0,
            volume_amplifier: 5.0,
            is_minecart: false,
        });
        let mut i = inst();
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 5.0, "not clamped here; the engine clamps the gain");
    }

    /// **The pair mutes by submersion, and the minecart asks the PLAYER while
    /// the base class asks the vehicle.** The two disagree exactly when a cart
    /// is submerged and its rider is not, which is most of the way in.
    #[test]
    fn the_underwater_pair_mutes_the_half_that_does_not_match() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        w.on_rails.push(2);
        w.horizontal_speed.insert(2, 1.0);
        // The cart is under water; the rider is not.
        w.underwater.push(2);

        let mut dry = riding(true, false);
        let mut wet = riding(true, true);
        let (mut di, mut wi) = (inst(), inst());
        dry.tick(&mut di, &w);
        wet.tick(&mut wi, &w);
        // A minecart reads the PLAYER, who is dry — so the dry loop plays.
        assert!((di.volume - 0.75).abs() < 1e-6, "{}", di.volume);
        assert_eq!(wi.volume, 0.0);

        // The base class reads the VEHICLE, which is submerged, so the same
        // world gives the opposite answer.
        let mut base_dry = riding(false, false);
        let mut base_wet = riding(false, true);
        w.speed.insert(2, 1.0);
        let (mut bdi, mut bwi) = (inst(), inst());
        base_dry.tick(&mut bdi, &w);
        base_wet.tick(&mut bwi, &w);
        assert_eq!(bdi.volume, 0.0);
        assert!((bwi.volume - 0.75).abs() < 1e-6, "{}", bwi.volume);
    }

    /// Dismounting stops it, and so does mounting something else.
    #[test]
    fn riding_stops_when_the_player_leaves_the_vehicle() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        let mut r = riding(false, false);
        assert!(!r.tick(&mut inst(), &w).stopped);

        w.vehicles.remove(&1);
        assert!(r.tick(&mut inst(), &w).stopped, "not a passenger");

        w.vehicles.insert(1, 99);
        assert!(r.tick(&mut inst(), &w).stopped, "a different vehicle");
    }

    /// **`shoudlPlaySound()` (vanilla's spelling) is a minecart-only override,
    /// and it is a disjunction.** A new-behaviour cart off its rails is silent;
    /// an old-behaviour one is not, however far from a rail it is; and a
    /// non-minecart rider never asks at all.
    ///
    /// Written because the battery found nothing covering it — every other
    /// riding fixture put the cart on rails, so deleting the whole predicate
    /// changed no assertion.
    #[test]
    fn a_minecart_rider_needs_rails_only_under_the_new_behaviour() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        w.horizontal_speed.insert(2, 1.0);
        w.speed.insert(2, 1.0);
        let mut r = riding(true, false);
        let mut i = inst();

        // New behaviour, off rails → the minimum, not the ramp.
        w.new_behavior.push(2);
        r.tick(&mut i, &w);
        assert_eq!(i.volume, 0.0);

        // Old behaviour, off rails → audible.
        w.new_behavior.clear();
        r.tick(&mut i, &w);
        assert!((i.volume - 0.75).abs() < 1e-6, "{}", i.volume);

        // New behaviour, on rails → audible.
        w.new_behavior.push(2);
        w.on_rails.push(2);
        r.tick(&mut i, &w);
        assert!((i.volume - 0.75).abs() < 1e-6, "{}", i.volume);

        // A non-minecart rider never asks: same world, off rails, audible.
        w.on_rails.clear();
        let mut ghast = riding(false, false);
        let mut gi = inst();
        ghast.tick(&mut gi, &w);
        assert!((gi.volume - 0.75).abs() < 1e-6, "{}", gi.volume);
    }

    /// A minecart rider reads the **horizontal** speed, so falling is quiet;
    /// the base class reads the full length, so it is not.
    #[test]
    fn a_minecart_rider_ignores_vertical_speed_and_the_base_class_does_not() {
        let mut w = World::at(2, (0.0, 64.0, 0.0));
        w.vehicles.insert(1, 2);
        w.on_rails.push(2);
        w.horizontal_speed.insert(2, 0.0);
        w.speed.insert(2, 1.0); // straight down

        let mut cart = riding(true, false);
        let mut ghast = riding(false, false);
        let (mut ci, mut gi) = (inst(), inst());
        cart.tick(&mut ci, &w);
        ghast.tick(&mut gi, &w);
        assert_eq!(ci.volume, 0.0, "horizontal only");
        assert!((gi.volume - 0.75).abs() < 1e-6, "{}", gi.volume);
    }

    // ---- the sniffer -----------------------------------------------------

    /// Two states keep it alive, not one: `DIGGING` **or** `SEARCHING`.
    #[test]
    fn the_sniffer_stops_when_it_is_not_digging() {
        let mut w = World::at(4, (0.0, 64.0, 0.0));
        w.digging.push(4);
        let mut r = Ramp::Sniffer { sniffer: 4 };
        let mut i = inst();
        assert!(!r.tick(&mut i, &w).stopped);
        assert_eq!((i.volume, i.pitch), (1.0, 1.0));

        w.digging.clear();
        assert!(r.tick(&mut i, &w).stopped);
    }

    // ---- underwater ------------------------------------------------------

    /// **+1 up, −2 down**, and the guard is read before the update — so the
    /// instance stops the tick *after* the fade goes negative.
    #[test]
    fn the_underwater_loop_fades_out_twice_as_fast_as_in() {
        let mut w = World::at(1, (0.0, 30.0, 0.0));
        w.underwater.push(1);
        let mut r = Ramp::UnderwaterLoop(UnderwaterRamp { player: 1, fade: 0 });
        let mut i = inst();

        // Forty ticks submerged to reach full volume.
        for _ in 0..40 {
            r.tick(&mut i, &w);
        }
        assert_eq!(i.volume, 1.0);
        let Ramp::UnderwaterLoop(u) = r else { unreachable!() };
        assert_eq!(u.fade, 40, "capped above at FADE_DURATION");

        // Twenty to fall back — half as many.
        w.underwater.clear();
        for _ in 0..20 {
            r.tick(&mut i, &w);
        }
        assert_eq!(i.volume, 0.0);
        let Ramp::UnderwaterLoop(u) = r else { unreachable!() };
        assert_eq!(u.fade, 0);

        // Now it goes negative — and still writes a volume this tick.
        assert!(!r.tick(&mut i, &w).stopped, "the guard is read BEFORE the step");
        let Ramp::UnderwaterLoop(u) = r else { unreachable!() };
        assert_eq!(u.fade, -2, "not clamped below");
        assert_eq!(i.volume, 0.0, "max(0, ..) keeps it non-negative");
        assert!(r.tick(&mut i, &w).stopped, "the NEXT tick sees fade < 0");
    }

    /// The cap is applied after the step, so a long dive does not bank credit
    /// it would have to spend on the way up.
    #[test]
    fn the_underwater_fade_does_not_bank_credit_above_the_cap() {
        let mut w = World::at(1, (0.0, 30.0, 0.0));
        w.underwater.push(1);
        let mut r = Ramp::UnderwaterLoop(UnderwaterRamp { player: 1, fade: 0 });
        let mut i = inst();
        for _ in 0..200 {
            r.tick(&mut i, &w);
        }
        let Ramp::UnderwaterLoop(u) = r else { unreachable!() };
        assert_eq!(u.fade, 40, "200 ticks of +1 still caps at 40");
    }

    /// `SubSound` has no ramp at all — only the stop condition.
    #[test]
    fn the_underwater_sub_sound_only_stops() {
        let mut w = World::at(1, (0.0, 30.0, 0.0));
        w.underwater.push(1);
        let mut r = Ramp::UnderwaterSub { player: 1 };
        let mut i = inst();
        let before = i.clone();
        assert!(!r.tick(&mut i, &w).stopped);
        assert_eq!(i, before, "nothing is written");
        w.underwater.clear();
        assert!(r.tick(&mut i, &w).stopped);
    }

    // ---- the biome loop --------------------------------------------------

    /// **No `else`** — a stopped loop still advances its fade and still writes
    /// a volume on the tick it stops.
    #[test]
    fn a_stopped_biome_loop_still_ramps_on_the_tick_it_stops() {
        let mut r = Ramp::BiomeLoop(BiomeLoopRamp {
            fade: -1,
            fade_direction: 1,
        });
        let mut i = inst();
        let out = r.tick(&mut i, &());
        assert!(out.stopped);
        let Ramp::BiomeLoop(b) = r else { unreachable!() };
        assert_eq!(b.fade, 0, "the step ran anyway");
        assert_eq!(i.volume, 0.0, "and so did the write");
    }

    /// **`fadeDirection` starts at 0**, so a fresh loop is frozen until
    /// something calls `fadeIn`/`fadeOut`.
    #[test]
    fn a_fresh_biome_loop_never_moves() {
        let mut r = Ramp::BiomeLoop(BiomeLoopRamp::default_for_test());
        let mut i = inst();
        for _ in 0..100 {
            assert!(!r.tick(&mut i, &()).stopped);
        }
        let Ramp::BiomeLoop(b) = r else { unreachable!() };
        assert_eq!(b.fade, 0);
        assert_eq!(i.volume, 0.0);
    }

    /// `fadeIn` floors a negative fade at 0; `fadeOut` caps an over-large one
    /// at 40. Not the same operation mirrored.
    #[test]
    fn fade_in_floors_and_fade_out_caps() {
        let mut b = BiomeLoopRamp {
            fade: -5,
            fade_direction: -1,
        };
        b.fade_in();
        assert_eq!((b.fade, b.fade_direction), (0, 1));

        let mut b = BiomeLoopRamp {
            fade: 999,
            fade_direction: 1,
        };
        b.fade_out();
        assert_eq!((b.fade, b.fade_direction), (40, -1));

        // …and neither touches a value already inside the band.
        let mut b = BiomeLoopRamp {
            fade: 17,
            fade_direction: 0,
        };
        b.fade_in();
        assert_eq!(b.fade, 17);
        b.fade_out();
        assert_eq!(b.fade, 17);
    }

    impl BiomeLoopRamp {
        fn default_for_test() -> BiomeLoopRamp {
            BiomeLoopRamp {
                fade: 0,
                fade_direction: 0,
            }
        }
    }

    /// A world with nothing in it, for the two ramps that ask it nothing.
    impl RampWorld for () {
        fn position(&self, _: i32) -> Option<(f64, f64, f64)> {
            None
        }
        fn horizontal_speed(&self, _: i32) -> f64 {
            0.0
        }
        fn speed(&self, _: i32) -> f64 {
            0.0
        }
        fn speed_sqr(&self, _: i32) -> f64 {
            0.0
        }
        fn baby(&self, _: i32) -> bool {
            false
        }
        fn angry(&self, _: i32) -> bool {
            false
        }
        fn underwater(&self, _: i32) -> bool {
            false
        }
        fn has_ai_target(&self, _: i32) -> bool {
            false
        }
        fn attack_animation_scale(&self, _: i32) -> f32 {
            0.0
        }
        fn sniffer_digging(&self, _: i32) -> bool {
            false
        }
        fn on_rails(&self, _: i32) -> bool {
            false
        }
        fn new_minecart_behavior(&self, _: i32) -> bool {
            false
        }
        fn runs_normally(&self) -> bool {
            true
        }
        fn fall_flying(&self, _: i32) -> bool {
            false
        }
        fn vehicle_of(&self, _: i32) -> Option<i32> {
            None
        }
        fn camera_position(&self) -> (f64, f64, f64) {
            (0.0, 0.0, 0.0)
        }
    }

    // ---- directional -----------------------------------------------------

    /// Ten blocks from the camera along the bearing, re-read every tick, with
    /// `Attenuation.NONE` reasserted.
    #[test]
    fn a_directional_sound_sits_ten_blocks_out_and_follows_the_camera() {
        struct Cam(std::cell::Cell<(f64, f64, f64)>);
        impl RampWorld for Cam {
            fn position(&self, _: i32) -> Option<(f64, f64, f64)> {
                None
            }
            fn horizontal_speed(&self, _: i32) -> f64 {
                0.0
            }
            fn speed(&self, _: i32) -> f64 {
                0.0
            }
            fn speed_sqr(&self, _: i32) -> f64 {
                0.0
            }
            fn baby(&self, _: i32) -> bool {
                false
            }
            fn angry(&self, _: i32) -> bool {
                false
            }
            fn underwater(&self, _: i32) -> bool {
                false
            }
            fn has_ai_target(&self, _: i32) -> bool {
                false
            }
            fn attack_animation_scale(&self, _: i32) -> f32 {
                0.0
            }
            fn sniffer_digging(&self, _: i32) -> bool {
                false
            }
            fn on_rails(&self, _: i32) -> bool {
                false
            }
            fn new_minecart_behavior(&self, _: i32) -> bool {
                false
            }
            fn runs_normally(&self) -> bool {
                true
            }
            fn fall_flying(&self, _: i32) -> bool {
                false
            }
            fn vehicle_of(&self, _: i32) -> Option<i32> {
                None
            }
            fn camera_position(&self) -> (f64, f64, f64) {
                self.0.get()
            }
        }

        let cam = Cam(std::cell::Cell::new((100.0, 70.0, -20.0)));
        let mut r = Ramp::Directional {
            x_angle: 0.0,
            y_angle: 0.0,
        };
        let mut i = inst();
        r.tick(&mut i, &cam);
        // The distance from the camera is 10 regardless of the bearing.
        let d = ((i.x - 100.0).powi(2) + (i.y - 70.0).powi(2) + (i.z + 20.0).powi(2)).sqrt();
        assert!((d - 10.0).abs() < 1e-3, "distance {d}");
        assert_eq!(i.attenuation, Attenuation::None);

        // …and it moves with the camera rather than staying put.
        let first = (i.x, i.y, i.z);
        cam.0.set((0.0, 70.0, -20.0));
        r.tick(&mut i, &cam);
        assert_ne!((i.x, i.y, i.z), first);
        let d2 = ((i.x - 0.0).powi(2) + (i.y - 70.0).powi(2) + (i.z + 20.0).powi(2)).sqrt();
        assert!((d2 - 10.0).abs() < 1e-3, "distance {d2}");
    }

    /// `Entity.calculateViewVector` — an independent transcription, so
    /// agreement is evidence rather than a tautology. M138a used exactly this
    /// oracle against `listener_basis`; this is the third expression in the
    /// codebase that turns out to compute it.
    ///
    /// ```java
    /// float f = pitch * (pi/180), g = -yaw * (pi/180);
    /// return new Vec3(sin(g) * cos(f), -sin(f), cos(g) * cos(f));
    /// ```
    fn view_vector(pitch_deg: f32, yaw_deg: f32) -> (f64, f64, f64) {
        let f = pitch_deg.to_radians();
        let g = -yaw_deg.to_radians();
        (
            (g.sin() * f.cos()) as f64,
            (-f.sin()) as f64,
            (g.cos() * f.cos()) as f64,
        )
    }

    /// **`Vec3.directionFromRotation` IS `Entity.calculateViewVector`**, by a
    /// different expression — the four negations cancel pairwise.
    ///
    /// ```text
    /// yCos = cos(g - pi) = -cos(g)      xCos = -cos(-f) = -cos(f)
    /// ySin = sin(g - pi) = -sin(g)      xSin =  sin(-f) = -sin(f)
    /// (ySin*xCos, xSin, yCos*xCos) = (sin(g)cos(f), -sin(f), cos(g)cos(f))
    /// ```
    ///
    /// So the `- PI` in the yaw terms and the leading `-` on `xCos` are a
    /// matched pair: reproduce **both** and you get the ordinary view vector,
    /// reproduce **one** and every direction is reflected.
    ///
    /// **This witness was wrong before the code was**, which is the seventh
    /// such instance in this project's log and the second caused by reaching
    /// for a remembered note instead of evaluating the expression. It asserted
    /// yaw 0 points at **-Z**, "the same convention M138a records for
    /// `Camera`'s `FORWARDS`" — but M138a's note says the opposite: the raw
    /// `FORWARDS` constant is -Z and `setRotation`'s leading half-turn is what
    /// makes **yaw 0 face +Z**. Minecraft yaw 0 is south. Evaluating
    /// `cos(-pi) * -cos(0) = +1` settles it in one line.
    #[test]
    fn direction_from_rotation_is_the_view_vector_by_another_route() {
        // Not one angle: a dropped negation agrees at 0 and diverges
        // elsewhere, which is the fixture shape this repo keeps being caught
        // by. The tolerance is 2e-4 because `Mth`'s 65,536-entry sine table is
        // quantised where the oracle's platform trig is not — M12 records that
        // `Mth.sin(pi)` is even a tiny *positive* value where platform sine is
        // negative, so near-zero components can differ in sign.
        for pitch in [-90.0f32, -45.0, -12.5, 0.0, 17.0, 45.0, 90.0] {
            for yaw in [-180.0f32, -135.0, -90.0, -45.0, 0.0, 30.0, 90.0, 179.0] {
                let got = direction_from_rotation(pitch, yaw);
                let want = view_vector(pitch, yaw);
                for (i, (g, w)) in [(got.0, want.0), (got.1, want.1), (got.2, want.2)]
                    .into_iter()
                    .enumerate()
                {
                    assert!(
                        (g - w).abs() < 2e-4,
                        "pitch {pitch} yaw {yaw} component {i}: {g} vs {w}"
                    );
                }
            }
        }

        // The three cardinal readings, named so a reader does not have to run
        // the loop in their head. Yaw 0 is +Z (south), NOT -Z.
        let (x, y, z) = direction_from_rotation(0.0, 0.0);
        assert!(x.abs() < 2e-4 && y.abs() < 2e-4, "{x} {y}");
        assert!((z - 1.0).abs() < 2e-4, "z {z} — +1, not -1");
        // Pitch -90 (straight up) gives +Y.
        assert!((direction_from_rotation(-90.0, 0.0).1 - 1.0).abs() < 2e-4);
        // Yaw 90 (west) gives -X.
        assert!((direction_from_rotation(0.0, 90.0).0 + 1.0).abs() < 2e-4);
    }

    // ---- entity-bound ----------------------------------------------------

    /// The reference body, and the one M131 already had — kept here so all ten
    /// live together and the engine can drive one code path.
    #[test]
    fn an_entity_bound_ramp_follows_and_narrows() {
        let far = 1_000_000.1_f64;
        let w = World::at(7, (far, 64.0, 0.0));
        let mut r = Ramp::EntityBound { entity: 7 };
        let mut i = inst();
        assert!(!r.tick(&mut i, &w).stopped);
        assert_eq!(i.x, far as f32 as f64);
        assert_ne!(i.x, far, "the fixture must not sit where both agree");

        let empty = World::default();
        assert!(r.tick(&mut i, &empty).stopped);
    }

    /// **Exactly six of the ten are silence-gated**, and the set is not the
    /// set of ramps that follow an entity — which is the whole point of the
    /// two accessors being different.
    #[test]
    fn only_the_six_that_override_can_play_sound_are_silence_gated() {
        let riding = riding(true, false);
        let gated: Vec<(&str, Option<i32>)> = vec![
            ("entity_bound", Ramp::EntityBound { entity: 1 }.silence_gated_entity()),
            (
                "bee",
                Ramp::Bee(BeeRamp {
                    bee: 2,
                    loop_kind: BeeLoop::Flying,
                    has_switched: false,
                })
                .silence_gated_entity(),
            ),
            (
                "guardian",
                Ramp::Guardian {
                    guardian: 4,
                    attack_duration: 80,
                }
                .silence_gated_entity(),
            ),
            (
                "minecart",
                Ramp::Minecart(MinecartRamp {
                    minecart: 5,
                    shadowed_pitch: 0.0,
                })
                .silence_gated_entity(),
            ),
            ("riding", riding.silence_gated_entity()),
            ("sniffer", Ramp::Sniffer { sniffer: 6 }.silence_gated_entity()),
        ];
        for (what, g) in gated {
            assert!(g.is_some(), "{what} overrides canPlaySound and must gate");
        }

        // The four that take the interface default's flat `true`. An elytra
        // sound that a silenced player silenced would be Rewo's invention.
        let ungated: Vec<(&str, Option<i32>)> = vec![
            (
                "elytra",
                Ramp::Elytra(ElytraRamp { player: 3, time: 0 }).silence_gated_entity(),
            ),
            (
                "underwater loop",
                Ramp::UnderwaterLoop(UnderwaterRamp { player: 7, fade: 0 })
                    .silence_gated_entity(),
            ),
            (
                "underwater sub",
                Ramp::UnderwaterSub { player: 8 }.silence_gated_entity(),
            ),
            (
                "biome loop",
                Ramp::BiomeLoop(BiomeLoopRamp {
                    fade: 0,
                    fade_direction: 0,
                })
                .silence_gated_entity(),
            ),
            (
                "directional",
                Ramp::Directional {
                    x_angle: 0.0,
                    y_angle: 0.0,
                }
                .silence_gated_entity(),
            ),
        ];
        for (what, g) in ungated {
            assert_eq!(g, None, "{what} takes the default and must NOT gate");
        }

        // …and the two accessors disagree exactly where a class declines the
        // gate while still following something: the elytra follows its player
        // and is not gated on it.
        let elytra = Ramp::Elytra(ElytraRamp { player: 3, time: 0 });
        assert_eq!(elytra.entity(), Some(3));
        assert_eq!(elytra.silence_gated_entity(), None);
    }

    /// Every ramp reports the entity it needs, and the two that follow no
    /// entity say so — the engine uses this to decide what to look up.
    #[test]
    fn every_ramp_names_the_entity_it_follows() {
        assert_eq!(Ramp::EntityBound { entity: 1 }.entity(), Some(1));
        assert_eq!(
            Ramp::Bee(BeeRamp {
                bee: 2,
                loop_kind: BeeLoop::Flying,
                has_switched: false
            })
            .entity(),
            Some(2)
        );
        assert_eq!(
            Ramp::Elytra(ElytraRamp { player: 3, time: 0 }).entity(),
            Some(3)
        );
        assert_eq!(
            Ramp::Guardian {
                guardian: 4,
                attack_duration: 80
            }
            .entity(),
            Some(4)
        );
        assert_eq!(
            Ramp::Minecart(MinecartRamp {
                minecart: 5,
                shadowed_pitch: 0.0
            })
            .entity(),
            Some(5)
        );
        // Riding follows the VEHICLE, not the player — the vehicle is what
        // the sound is positioned by and what `isRemoved()` is asked about.
        let Ramp::Riding(r) = riding(true, false) else {
            unreachable!()
        };
        assert_eq!(Ramp::Riding(r).entity(), Some(r.vehicle));
        assert_eq!(Ramp::Sniffer { sniffer: 6 }.entity(), Some(6));
        assert_eq!(
            Ramp::UnderwaterLoop(UnderwaterRamp { player: 7, fade: 0 }).entity(),
            Some(7)
        );
        assert_eq!(Ramp::UnderwaterSub { player: 8 }.entity(), Some(8));
        assert_eq!(
            Ramp::BiomeLoop(BiomeLoopRamp {
                fade: 0,
                fade_direction: 0
            })
            .entity(),
            None
        );
        assert_eq!(
            Ramp::Directional {
                x_angle: 0.0,
                y_angle: 0.0
            }
            .entity(),
            None
        );
    }
}
