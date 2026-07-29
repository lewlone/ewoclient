//! M75 — `Abilities`: the local player's movement permissions, the flight
//! they unlock, and the client-side controller that toggles it.
//!
//! Ground truth is the decompiled 26.2 `world/entity/player/Abilities.java`,
//! `Player.travel` / `Player.getFlyingSpeed`, `LivingEntity.travelInAir`, and
//! `client/player/LocalPlayer.aiStep`. M71 modelled the local player's
//! *gamemode* but explicitly did not act on it, recording that `physics` had no
//! concept of flight, no-clip or invulnerability. This is that concept.
//!
//! # Three things here read backwards
//!
//! **1. `walkingSpeed` is not the client's walking speed.** It is tempting to
//! wire the packet's second float into the walk path, and at defaults it even
//! agrees (`0.1 == 0.1`). But on the client `getWalkingSpeed()` has exactly one
//! consumer — `AbstractClientPlayer.getFieldOfViewModifier`, which uses it as
//! the *divisor* for `Attributes.MOVEMENT_SPEED` to compute the speed-based FOV
//! stretch. Its only movement role, `Player.readAdditionalSaveData` seeding the
//! `MOVEMENT_SPEED` base value, is server-side NBT load. The client's walk speed
//! comes from that attribute, synced by its own packet. So a server that raises
//! one without the other would expose the mistake and nothing else would.
//!
//! **2. Flight has no gravity term, and its vertical drag is 0.6 — not 0.98.**
//! `Player.travel` does not call `travelFlying` (that is for mobs and for
//! swimming). It captures `originalMovementY`, delegates to the *ordinary*
//! `LivingEntity.travelInAir`, and then **overwrites** the Y it just computed
//! with `originalMovementY * 0.6`. `travelInAir`'s gravity subtraction and its
//! 0.98 vertical drag both still run — and are then discarded whole. The move
//! itself has already happened by then, using the pre-gravity velocity, so the
//! only surviving vertical effect is the ×0.6 decay.
//!
//! **3. Sneaking does not slow a flying player.** The 0.3 `SNEAKING_SPEED`
//! factor in `LocalPlayer.modifyInput` is gated on `isMovingSlowly()`, which is
//! `isCrouching()`, which reads the `crouching` field — and `LocalPlayer.aiStep`
//! assigns `crouching = !abilities.flying && ...`. While flying, the sneak key
//! is purely the descend input and costs no horizontal speed.
//!
//! # Scoped exclusions (recorded rather than guessed)
//!
//! - **A mounted player cannot toggle flight here.** Vanilla's guard is
//!   `getVehicle() == null || jumpableVehicle() != null`, so a rider on a horse
//!   may toggle and a rider in a boat may not. Rewo models neither
//!   `PlayerRideableJumping` nor a per-vehicle jump capability, so
//!   [`FlightControl::before_travel`] takes the boat arm for every vehicle. The
//!   safe subset, not a guess at the other one.
//! - **Fluids are not modelled.** `Player.travel`'s swimming pre-step and
//!   `travelInFluid` are outside M75; the flight path assumes air, which is
//!   where creative flight is used.
//! - **`instabuild` and `invulnerable` are stored and not acted on.** Nothing in
//!   Rewo does block-break timing or damage application from the client side,
//!   so acting on them would be inventing behaviour. `may_build` likewise.

use crate::physics::{PlayerState, TickInput};

/// `Abilities.DEFAULT_FLYING_SPEED`.
pub const DEFAULT_FLYING_SPEED: f32 = 0.05;
/// `Abilities.DEFAULT_WALKING_SPEED`.
pub const DEFAULT_WALKING_SPEED: f32 = 0.1;

/// `LocalPlayer.aiStep`: the tick count a first jump press arms the double-tap
/// window with. Vanilla writes the literal `7`.
pub const JUMP_TRIGGER_TICKS: i32 = 7;

/// `Player.travel`: the factor the whole vertical velocity is replaced by on
/// every flying tick. This *is* flight's vertical drag — see the module note.
///
/// `0.6` is a `double` literal in vanilla (`originalMovementY * 0.6`), so this
/// one really is f64 arithmetic — unlike the two below.
pub const FLYING_VERTICAL_DECAY: f64 = 0.6;

/// `LocalPlayer.aiStep`: the ascend/descend impulse is `flyingSpeed * 3`.
///
/// **f32 on purpose.** Vanilla's expression is `inputYa * getFlyingSpeed() *
/// 3.0F` — `int * float * float`, so the whole product is computed in **float**
/// and only widened when it reaches `Vec3.add(double, double, double)`.
/// Widening first and multiplying in f64 gives a different number: the default
/// 0.05 is not representable, so `0.05f as f64 * 3.0` is 0.15000000223…, while
/// the faithful `(0.05f * 3.0f) as f64` is 0.15000000596…. Same shape as M12's
/// `Mth.floor`-returns-an-`int` rule — narrow where Java is narrow.
pub const FLYING_VERTICAL_IMPULSE_MULT: f32 = 3.0;

/// `Player.getFlyingSpeed`: sprinting doubles the flying `moveRelative` amount.
pub const FLYING_SPRINT_MULT: f32 = 2.0;

/// The local player's `Abilities` — vanilla's field set, verbatim.
///
/// The two speeds are private with accessors because that is how vanilla holds
/// them (`getFlyingSpeed`/`setFlyingSpeed`), and because [`Self::walking_speed`]
/// is a trap worth making people ask for by name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Abilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub mayfly: bool,
    pub instabuild: bool,
    pub may_build: bool,
    flying_speed: f32,
    walking_speed: f32,
}

impl Default for Abilities {
    /// Vanilla's field initialisers: four `false`, `mayBuild = true`, and the
    /// two speed defaults. `may_build` defaulting to `true` is the one that is
    /// not simply "everything off".
    fn default() -> Self {
        Self {
            invulnerable: false,
            flying: false,
            mayfly: false,
            instabuild: false,
            may_build: true,
            flying_speed: DEFAULT_FLYING_SPEED,
            walking_speed: DEFAULT_WALKING_SPEED,
        }
    }
}

impl Abilities {
    /// `Abilities.getFlyingSpeed`.
    pub fn flying_speed(&self) -> f32 {
        self.flying_speed
    }

    /// `Abilities.setFlyingSpeed`.
    pub fn set_flying_speed(&mut self, v: f32) {
        self.flying_speed = v;
    }

    /// `Abilities.getWalkingSpeed`.
    ///
    /// **Not the client's walk speed** — see the module note. It is the FOV
    /// modifier's divisor. Rewo has no FOV modifier, so nothing reads this yet;
    /// it is stored because the packet carries it and dropping a decoded field
    /// is how a later reader ends up re-deriving it wrongly.
    pub fn walking_speed(&self) -> f32 {
        self.walking_speed
    }

    /// `Abilities.setWalkingSpeed`.
    pub fn set_walking_speed(&mut self, v: f32) {
        self.walking_speed = v;
    }

    /// `Player.getFlyingSpeed()` — the `moveRelative` amount used whenever the
    /// player is **airborne**, flying or not.
    ///
    /// Vanilla's override returns the abilities' flying speed (doubled while
    /// sprinting) when flying and not a passenger, and otherwise falls through
    /// to the ordinary air-control constants. Those two constants are the ones
    /// `physics` already had, so this is the single place both live.
    pub fn air_move_speed(&self, sprinting: bool, flying: bool) -> f64 {
        if flying {
            let s = if sprinting {
                self.flying_speed * FLYING_SPRINT_MULT
            } else {
                self.flying_speed
            };
            s as f64
        } else if sprinting {
            crate::physics::AIR_SPEED_SPRINT
        } else {
            crate::physics::AIR_SPEED
        }
    }

    /// The vertical impulse added on a flying tick: `inputYa * flyingSpeed * 3`,
    /// where `inputYa` is `+1` for jump, `-1` for sneak, and `0` when both or
    /// neither are held — vanilla increments and decrements the same `int`.
    ///
    /// Computed in f32 and widened last; see [`FLYING_VERTICAL_IMPULSE_MULT`].
    /// The `inputYa == 0` early return is vanilla's `if (inputYa != 0)` guard,
    /// which matters only in that it skips the `setDeltaMovement` call.
    pub fn vertical_flight_impulse(&self, jump: bool, sneak: bool) -> f64 {
        let input_ya = i32::from(jump) - i32::from(sneak);
        if input_ya == 0 {
            return 0.0;
        }
        f64::from(input_ya as f32 * self.flying_speed * FLYING_VERTICAL_IMPULSE_MULT)
    }
}

/// The four ability flags a gamemode dictates, plus `may_build`.
///
/// This is `GameType.updatePlayerAbilities`'s payload, expressed so the mapping
/// can live beside `GameMode` (in `rewo-net`, where the enum is) without this
/// crate having to know the enum exists. `rewo-net` builds one and applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeAbilities {
    pub mayfly: bool,
    pub instabuild: bool,
    pub invulnerable: bool,
    /// `Some(v)` where the mode assigns `flying`, `None` where it leaves it
    /// alone.
    ///
    /// **This `Option` is the whole asymmetry.** `GameType.updatePlayerAbilities`
    /// sets `flying = true` for SPECTATOR and `flying = false` for
    /// survival/adventure, but **says nothing about `flying` for CREATIVE** —
    /// entering creative does not start you flying. Collapsing this to a `bool`
    /// forces a choice that is wrong for exactly one mode, and it is the mode a
    /// tester is most likely to be in: you would switch to creative, press the
    /// fly key, and never notice the initial state had been wrong.
    pub flying: Option<bool>,
    pub may_build: bool,
}

impl Abilities {
    /// The assignment half of `GameType.updatePlayerAbilities(Abilities)`.
    pub fn apply_mode(&mut self, m: ModeAbilities) {
        self.mayfly = m.mayfly;
        self.instabuild = m.instabuild;
        self.invulnerable = m.invulnerable;
        if let Some(f) = m.flying {
            self.flying = f;
        }
        self.may_build = m.may_build;
    }
}

/// What [`FlightControl::before_travel`] did this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlightStep {
    /// The client changed `abilities.flying` itself, so it owes the server a
    /// `ServerboundPlayerAbilitiesPacket` — vanilla's `onUpdateAbilities()`.
    pub abilities_changed: bool,
    /// The toggle turned flight **on** while standing, so vanilla's
    /// `jumpFromGround()` fires this tick. Reported rather than applied here
    /// because the jump impulse lives in `physics`.
    pub jump_from_ground: bool,
}

/// `LocalPlayer`'s client-side flight controller.
///
/// Holds the two pieces of per-player state the toggle needs: `jumpTriggerTime`
/// (`Player`'s field, decremented in `Player.aiStep`) and the previous tick's
/// jump key, which is vanilla's `wasJumping` local — captured at the *top* of
/// `aiStep`, before `input.tick()`, so the edge test compares last tick's key
/// against this tick's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlightControl {
    jump_trigger_time: i32,
    was_jumping: bool,
}

impl FlightControl {
    /// `jumpTriggerTime`, exposed for the gate. Vanilla arms it at
    /// [`JUMP_TRIGGER_TICKS`] and counts down one per tick.
    pub fn jump_trigger_time(&self) -> i32 {
        self.jump_trigger_time
    }

    /// Everything `LocalPlayer.aiStep` does *before* `super.aiStep()`, plus
    /// `Player.aiStep`'s leading `jumpTriggerTime--`.
    ///
    /// Order is vanilla's and it matters: the toggle reads `jumpTriggerTime`
    /// **before** the decrement, and the vertical impulse is added to the
    /// velocity **before** `travel` runs — so the impulse is part of the
    /// distance moved this tick, not just of the velocity carried into the next
    /// one.
    ///
    /// `spectator` forces flight on and is never toggled off; `mounted` takes
    /// the non-jumpable-vehicle arm (see the module note).
    pub fn before_travel(
        &mut self,
        ab: &mut Abilities,
        st: &mut PlayerState,
        input: &TickInput,
        spectator: bool,
        mounted: bool,
    ) -> FlightStep {
        let was_jumping = self.was_jumping;
        self.was_jumping = input.jump;
        let mut step = FlightStep::default();

        if ab.mayfly {
            if spectator {
                // A spectator is *always* flying; vanilla asserts it every tick
                // rather than trusting the gamemode assignment to have run.
                if !ab.flying {
                    ab.flying = true;
                    step.abilities_changed = true;
                }
            } else if !was_jumping && input.jump {
                if self.jump_trigger_time == 0 {
                    // First press: arm the window. No toggle.
                    self.jump_trigger_time = JUMP_TRIGGER_TICKS;
                } else if !mounted {
                    ab.flying = !ab.flying;
                    if ab.flying && st.on_ground {
                        step.jump_from_ground = true;
                    }
                    step.abilities_changed = true;
                    self.jump_trigger_time = 0;
                }
            }
        }

        // `Player.aiStep`'s first statement, which runs inside the
        // `super.aiStep()` the toggle above precedes.
        if self.jump_trigger_time > 0 {
            self.jump_trigger_time -= 1;
        }

        if ab.flying {
            st.vy += ab.vertical_flight_impulse(input.jump, input.sneak);
        }

        step
    }

    /// `LocalPlayer.aiStep`'s tail: flight ends the moment you touch the ground
    /// — **except** in spectator, which is excluded by name.
    ///
    /// Returns whether the client changed `flying` and so owes the server a
    /// packet.
    pub fn after_travel(&mut self, ab: &mut Abilities, st: &PlayerState, spectator: bool) -> bool {
        if st.on_ground && ab.flying && !spectator {
            ab.flying = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_vanillas_field_initialisers() {
        let a = Abilities::default();
        assert!(!a.invulnerable && !a.flying && !a.mayfly && !a.instabuild);
        // The one that is not "everything off".
        assert!(a.may_build, "mayBuild initialises to true");
        assert_eq!(a.flying_speed(), 0.05);
        assert_eq!(a.walking_speed(), 0.1);
    }

    #[test]
    fn flying_speed_doubles_when_sprinting_and_air_speed_does_not() {
        let a = Abilities::default();
        // f32 values widened, not f64 literals — `getFlyingSpeed()` is a float
        // and `moveRelative` takes a float.
        assert_eq!(a.air_move_speed(false, true), f64::from(0.05f32));
        assert_eq!(a.air_move_speed(true, true), f64::from(0.05f32 * 2.0));
        // Not flying: the ordinary air-control constants, which are NOT a
        // doubling — 0.02 → 0.026, not 0.04.
        assert_eq!(a.air_move_speed(false, false), crate::physics::AIR_SPEED);
        assert_eq!(a.air_move_speed(true, false), crate::physics::AIR_SPEED_SPRINT);
        assert!(
            crate::physics::AIR_SPEED_SPRINT < crate::physics::AIR_SPEED * 2.0,
            "the walking air constants are not a doubling"
        );
    }

    #[test]
    fn vertical_impulse_cancels_when_both_keys_are_held() {
        let a = Abilities::default();
        // The f32 product: 0.05f × 3f rounds to exactly 0.15f, which is
        // 0.15000000596… as an f64 — *not* the f64 literal 0.15, and not the
        // 0.15000000223… that widening-first would give.
        let want = f64::from(0.15f32);
        assert_eq!(a.vertical_flight_impulse(true, false), want);
        assert_eq!(a.vertical_flight_impulse(false, true), -want);
        assert!(
            (want - 0.15f64).abs() > 1e-9,
            "the f32 path is observably different from the f64 one"
        );
        // vanilla increments then decrements one int, so both held is zero —
        // not "jump wins".
        assert_eq!(a.vertical_flight_impulse(true, true), 0.0);
        assert_eq!(a.vertical_flight_impulse(false, false), 0.0);
    }

    /// The double-tap window, measured rather than asserted from the literal 7.
    ///
    /// The literal is 7 but the usable window is **five** ticks of separation,
    /// because the decrement in `Player.aiStep` runs on the arming tick too: the
    /// counter is already 6 when the next tick starts, and a press only toggles
    /// while it is non-zero. The gap also cannot be 0 — the key must be released
    /// for there to be a second rising edge at all.
    #[test]
    fn double_tap_window_is_five_ticks_of_separation() {
        let toggles_after = |gap: usize| {
            let mut fc = FlightControl::default();
            let mut ab = Abilities {
                mayfly: true,
                ..Default::default()
            };
            let mut st = PlayerState::at(0.0, 0.0, 0.0);
            let press = TickInput {
                jump: true,
                ..Default::default()
            };
            let idle = TickInput::default();
            // Tick 0: first press (rising edge — `was_jumping` starts false).
            fc.before_travel(&mut ab, &mut st, &press, false, false);
            // Release for `gap` ticks, then press again.
            for _ in 0..gap {
                fc.before_travel(&mut ab, &mut st, &idle, false, false);
            }
            fc.before_travel(&mut ab, &mut st, &press, false, false);
            ab.flying
        };
        for gap in 1..=5 {
            assert!(toggles_after(gap), "a second press {gap} tick(s) later must toggle");
        }
        assert!(
            !toggles_after(6),
            "6 ticks later the counter has reached 0 — that press re-arms instead"
        );
        assert!(!toggles_after(20), "and stays closed after that");
    }

    /// A *held* jump key is one rising edge, so it can never double-tap.
    #[test]
    fn holding_jump_never_toggles_flight() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        for _ in 0..40 {
            fc.before_travel(&mut ab, &mut st, &press, false, false);
        }
        assert!(!ab.flying);
    }

    /// Without `mayfly` the whole block is skipped — the window never even arms.
    #[test]
    fn no_mayfly_means_no_toggle_and_no_window() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities::default();
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        let idle = TickInput::default();
        for _ in 0..4 {
            fc.before_travel(&mut ab, &mut st, &press, false, false);
            fc.before_travel(&mut ab, &mut st, &idle, false, false);
        }
        assert!(!ab.flying);
        assert_eq!(fc.jump_trigger_time(), 0, "the window never armed");
    }

    #[test]
    fn spectator_is_forced_flying_and_never_lands() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        let step = fc.before_travel(&mut ab, &mut st, &TickInput::default(), true, false);
        assert!(ab.flying && step.abilities_changed);
        st.on_ground = true;
        assert!(!fc.after_travel(&mut ab, &st, true), "spectator is excluded by name");
        assert!(ab.flying);
    }

    #[test]
    fn creative_flight_ends_on_landing() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            flying: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        st.on_ground = false;
        assert!(!fc.after_travel(&mut ab, &st, false), "airborne: nothing happens");
        st.on_ground = true;
        assert!(fc.after_travel(&mut ab, &st, false));
        assert!(!ab.flying);
    }

    #[test]
    fn toggling_on_while_standing_reports_a_jump() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        st.on_ground = true;
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        let idle = TickInput::default();
        let double_tap = |fc: &mut FlightControl, ab: &mut Abilities, st: &mut PlayerState| {
            fc.before_travel(ab, st, &press, false, false);
            fc.before_travel(ab, st, &idle, false, false);
            fc.before_travel(ab, st, &press, false, false)
        };
        let step = double_tap(&mut fc, &mut ab, &mut st);
        assert!(ab.flying);
        assert!(step.jump_from_ground, "`jumpFromGround()` fires on a standing toggle");

        // Toggling back OFF needs its own full double-tap: the ON toggle reset
        // `jumpTriggerTime` to 0, so the very next press *re-arms* rather than
        // toggling. And the OFF toggle does not jump, though we still stand.
        fc.before_travel(&mut ab, &mut st, &idle, false, false);
        let off = double_tap(&mut fc, &mut ab, &mut st);
        assert!(!ab.flying);
        assert!(!off.jump_from_ground);
    }

    /// The toggle resets the counter, so a *third* press straight after one
    /// cannot ride the previous window — it starts a new one.
    #[test]
    fn a_toggle_resets_the_window() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        let idle = TickInput::default();
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        fc.before_travel(&mut ab, &mut st, &idle, false, false);
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        assert!(ab.flying);
        assert_eq!(fc.jump_trigger_time(), 0, "the toggle zeroed it");
        // One release, one press — inside the *old* window's span, but it only
        // re-arms.
        fc.before_travel(&mut ab, &mut st, &idle, false, false);
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        assert!(ab.flying, "still flying — that press did not toggle back");
        assert_eq!(fc.jump_trigger_time(), JUMP_TRIGGER_TICKS - 1, "it re-armed");
    }

    #[test]
    fn a_mounted_player_cannot_toggle() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 0.0, 0.0);
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        fc.before_travel(&mut ab, &mut st, &press, false, true);
        fc.before_travel(&mut ab, &mut st, &TickInput::default(), false, true);
        fc.before_travel(&mut ab, &mut st, &press, false, true);
        assert!(!ab.flying);
    }

    #[test]
    fn apply_mode_leaves_flying_alone_when_the_mode_says_nothing() {
        let mut a = Abilities {
            flying: true,
            ..Default::default()
        };
        a.apply_mode(ModeAbilities {
            mayfly: true,
            instabuild: true,
            invulnerable: true,
            flying: None,
            may_build: true,
        });
        assert!(a.flying, "a `None` must not clear it");
        a.apply_mode(ModeAbilities {
            mayfly: false,
            instabuild: false,
            invulnerable: false,
            flying: Some(false),
            may_build: true,
        });
        assert!(!a.flying);
    }

    /// The impulse rides the velocity, so it is visible in the very next
    /// position — not deferred a tick.
    #[test]
    fn the_impulse_lands_before_travel_not_after() {
        let mut fc = FlightControl::default();
        let mut ab = Abilities {
            mayfly: true,
            flying: true,
            ..Default::default()
        };
        let mut st = PlayerState::at(0.0, 64.0, 0.0);
        assert_eq!(st.vy, 0.0);
        fc.before_travel(
            &mut ab,
            &mut st,
            &TickInput {
                jump: true,
                ..Default::default()
            },
            false,
            false,
        );
        assert_eq!(st.vy, f64::from(0.15f32), "vy carries the impulse into travel");
    }
}
