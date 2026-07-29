//! The server's tick clock (M74): `ticking_state` (127) and `ticking_step`
//! (128).
//!
//! `/tick freeze`, `/tick rate <n>` and `/tick step <n>` are the three
//! commands behind these two packets, and Rewo assumed a hard 20 Hz with no
//! way to learn otherwise. `REWO_PACKET_COVERAGE.md` classed both **A**: the
//! decode changes numbers, not pixels.
//!
//! **Decode and state only**, in M67's sense. [`TickRateManager`] is a
//! faithful port of vanilla's, including [`TickRateManager::tick`], but
//! nothing in `PlaySession`'s 20 Hz loop *consults* it yet. Gating the loop is
//! a behaviour change with live consequences for every existing harness
//! (`rewo play`'s correction meter times its movement against a 50 ms tick),
//! so it wants its own live gate rather than a free ride on this one. What
//! this milestone buys is that the numbers are true when someone wires them.
//!
//! ## The bodies
//!
//! | packet | body |
//! |---|---|
//! | `ticking_state` (127) | **f32** `tickRate`, then a **bool** `isFrozen` |
//! | `ticking_step` (128) | one **VarInt** `tickSteps` |
//!
//! `ticking_state` is `Packet.codec` over `FriendlyByteBuf`, i.e. a plain
//! `readFloat()` + `readBoolean()` — big-endian IEEE-754 then one byte. It is
//! *not* a var-int pair, and the float is not scaled.
//!
//! ## Three rules where the plausible implementation is silently wrong
//!
//! Each is mutation-tested by a witness below.
//!
//! 1. **`setTickRate` clamps up to `MIN_TICKRATE` (1.0), and the clamp is
//!    Java's `Math.max`.** Rust's `f32::max` and Java's `Math.max` disagree on
//!    `NaN`: Rust returns the other operand, Java propagates the `NaN`. The
//!    wire carries an unvalidated `f32`, so `NaN` is reachable from a
//!    malformed server, and the two readings give a client that runs at 1 Hz
//!    versus one whose tick length is `0`.
//! 2. **`nanosecondsPerTick` is derived, not stored twice.** It is
//!    `(long)(1e9 / tickrate)` computed from the *clamped* rate. Deriving it
//!    from the raw rate lets a rate below 1.0 produce a tick longer than a
//!    second even though the rate itself was clamped.
//! 3. **`tick()` reads `frozenTicksToRun` before decrementing it.**
//!    `runGameElements = !isFrozen || frozenTicksToRun > 0` and only then does
//!    the counter fall. Decrementing first loses exactly one stepped tick per
//!    `/tick step`, which for `step 1` means it does nothing at all.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/world/TickRateManager.java`
//! - `net/minecraft/network/protocol/game/ClientboundTickingStatePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundTickingStepPacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleTickingState`, `handleTickingStep`

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `TickRateManager.MIN_TICKRATE`.
pub const MIN_TICKRATE: f32 = 1.0;

/// `TimeUtil.NANOSECONDS_PER_SECOND`.
pub const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;

/// `TickRateManager` — the server's tick rate and freeze state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickRateManager {
    tickrate: f32,
    nanoseconds_per_tick: i64,
    frozen_ticks_to_run: i32,
    run_game_elements: bool,
    is_frozen: bool,
}

impl Default for TickRateManager {
    /// Vanilla's field initialisers: 20 Hz, 50 ms, not frozen, running.
    fn default() -> Self {
        Self {
            tickrate: 20.0,
            nanoseconds_per_tick: NANOSECONDS_PER_SECOND / 20,
            frozen_ticks_to_run: 0,
            run_game_elements: true,
            is_frozen: false,
        }
    }
}

impl TickRateManager {
    /// `setTickRate` — `Math.max(rate, 1.0F)`, then re-derive the tick length.
    /// Rules 1 and 2.
    pub fn set_tick_rate(&mut self, rate: f32) {
        // Java's `Math.max(float, float)` returns NaN if either argument is
        // NaN; Rust's `f32::max` returns the non-NaN one. Spelled out so the
        // two agree — see rule 1.
        self.tickrate = if rate.is_nan() { rate } else { rate.max(MIN_TICKRATE) };
        // `(long)((double)NANOSECONDS_PER_SECOND / this.tickrate)` — a double
        // divide narrowed by a Java cast, which truncates toward zero and
        // yields 0 for NaN. Rust's `as i64` saturates and maps NaN to 0, which
        // is the same on every value this can produce.
        self.nanoseconds_per_tick =
            (NANOSECONDS_PER_SECOND as f64 / f64::from(self.tickrate)) as i64;
    }

    /// `setFrozen`.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.is_frozen = frozen;
    }

    /// `setFrozenTicksToRun` — assigns, never accumulates. Two `/tick step 5`
    /// in a row leave five steps queued, not ten.
    pub fn set_frozen_ticks_to_run(&mut self, timeout: i32) {
        self.frozen_ticks_to_run = timeout;
    }

    /// `tick()` — recompute `runGameElements`, then burn one stepped tick.
    /// Rule 3: the read happens first.
    pub fn tick(&mut self) {
        self.run_game_elements = !self.is_frozen || self.frozen_ticks_to_run > 0;
        if self.frozen_ticks_to_run > 0 {
            self.frozen_ticks_to_run -= 1;
        }
    }

    /// `tickrate()`.
    pub fn tickrate(&self) -> f32 {
        self.tickrate
    }

    /// `nanosecondsPerTick()`.
    pub fn nanoseconds_per_tick(&self) -> i64 {
        self.nanoseconds_per_tick
    }

    /// `millisecondsPerTick()` — `(float)nanos / (float)NANOSECONDS_PER_MILLISECOND`.
    pub fn milliseconds_per_tick(&self) -> f32 {
        self.nanoseconds_per_tick as f32 / 1_000_000.0
    }

    /// `runsNormally()` — whether game elements ticked on the last [`Self::tick`].
    pub fn runs_normally(&self) -> bool {
        self.run_game_elements
    }

    /// `isFrozen()`.
    pub fn is_frozen(&self) -> bool {
        self.is_frozen
    }

    /// `isSteppingForward()` — `frozenTicksToRun > 0`.
    pub fn is_stepping_forward(&self) -> bool {
        self.frozen_ticks_to_run > 0
    }

    /// `frozenTicksToRun()`.
    pub fn frozen_ticks_to_run(&self) -> i32 {
        self.frozen_ticks_to_run
    }
}

/// Which of the two packets a body is. `ticking_step`'s body is a bare VarInt
/// and nothing in it says which packet it belongs to, so the id is the only
/// discriminator — the view area's radius/simulation pair again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickingPacket {
    State,
    Step,
}

/// The two resolved ids, lifted out of [`crate::ids::Ids`] so [`kind_for_id`]
/// and [`apply`] can be driven without building a full `Ids`.
#[derive(Clone, Copy, Debug)]
pub struct TickingIds {
    pub state: i32,
    pub step: i32,
}

/// Map an incoming packet id onto a kind, or `None` if it is neither.
pub fn kind_for_id(id: i32, ids: TickingIds) -> Option<TickingPacket> {
    if id == ids.state {
        Some(TickingPacket::State)
    } else if id == ids.step {
        Some(TickingPacket::Step)
    } else {
        None
    }
}

/// Decode one body and apply it. Returns whether the body decoded.
pub fn apply(kind: TickingPacket, body: &[u8], manager: &mut TickRateManager) -> bool {
    match kind {
        TickingPacket::State => match read_ticking_state(body) {
            Ok((rate, frozen)) => {
                // `handleTickingState` sets **both**, in this order.
                manager.set_tick_rate(rate);
                manager.set_frozen(frozen);
                log::debug!("net: ticking_state rate={rate} frozen={frozen}");
                true
            }
            Err(err) => {
                log::debug!("net: ticking_state decode: {err}");
                false
            }
        },
        TickingPacket::Step => match read_ticking_step(body) {
            Ok(steps) => {
                manager.set_frozen_ticks_to_run(steps);
                log::debug!("net: ticking_step {steps}");
                true
            }
            Err(err) => {
                log::debug!("net: ticking_step decode: {err}");
                false
            }
        },
    }
}

/// `ClientboundTickingStatePacket` — `readFloat()` then `readBoolean()`.
pub fn read_ticking_state(body: &[u8]) -> Result<(f32, bool)> {
    let mut r = PacketReader::new(body);
    let rate = r.f32()?;
    let frozen = r.bool()?;
    Ok((rate, frozen))
}

/// `ClientboundTickingStepPacket` — one VarInt.
pub fn read_ticking_step(body: &[u8]) -> Result<i32> {
    let mut r = PacketReader::new(body);
    r.varint()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ticking_state` is a big-endian f32 followed by one boolean byte — not
    /// a var-int, and the fields are in that order.
    ///
    /// MUTATION: reading the bool first. `20.0f32` is `0x41A00000`, whose
    /// leading byte `0x41` is a *truthy* boolean, so a swapped reader still
    /// decodes without error and reports a frozen world running at garbage.
    #[test]
    fn ticking_state_is_a_float_then_a_bool_in_that_order() {
        let mut body = 20.0f32.to_be_bytes().to_vec();
        body.push(0x01);
        assert_eq!(read_ticking_state(&body).unwrap(), (20.0, true));

        let mut body = 0.5f32.to_be_bytes().to_vec();
        body.push(0x00);
        assert_eq!(read_ticking_state(&body).unwrap(), (0.5, false));

        // Four bytes is not a whole body: the bool is required.
        assert!(read_ticking_state(&20.0f32.to_be_bytes()).is_err());
    }

    /// `ticking_step` is a VarInt.
    ///
    /// MUTATION: `r.varlong()? as i32`, the reading M67's survivor showed a
    /// length-only witness cannot distinguish — so the overlong case is
    /// asserted explicitly.
    #[test]
    fn ticking_step_is_a_var_int_and_rejects_an_overlong_one() {
        assert_eq!(read_ticking_step(&[0x80, 0x01]).unwrap(), 128);
        assert!(read_ticking_step(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).is_err());
    }

    /// Rule 1. The rate clamps **up** to 1.0, and the sample sits exactly on
    /// the bound as well as either side of it, because a `<` / `<=` slip in
    /// the clamp is invisible unless something is measured at 1.0 itself.
    ///
    /// MUTATION: dropping the clamp, or clamping *down* to 1.0 (i.e. reading
    /// `MIN_TICKRATE` as a maximum), which pins a normal 20 Hz server to 1 Hz.
    #[test]
    fn the_tick_rate_clamps_up_to_one() {
        let mut m = TickRateManager::default();
        m.set_tick_rate(0.25);
        assert_eq!(m.tickrate(), 1.0, "below the bound clamps up");
        m.set_tick_rate(1.0);
        assert_eq!(m.tickrate(), 1.0, "exactly on the bound is unchanged");
        m.set_tick_rate(20.0);
        assert_eq!(m.tickrate(), 20.0, "above the bound passes through");
        m.set_tick_rate(0.0);
        assert_eq!(m.tickrate(), 1.0);
        m.set_tick_rate(-5.0);
        assert_eq!(m.tickrate(), 1.0, "and a negative rate is not a fast one");
    }

    /// Rule 1's edge. Java's `Math.max` propagates `NaN`; Rust's `f32::max`
    /// swallows it. The wire carries an unvalidated float, so this is
    /// reachable from a malformed or hostile server.
    ///
    /// MUTATION: writing `rate.max(MIN_TICKRATE)` without the `is_nan` guard.
    /// That returns `1.0`, which is a *plausible* answer — which is exactly
    /// why the divergence would never be noticed.
    #[test]
    fn a_nan_tick_rate_propagates_the_way_java_max_does() {
        let mut m = TickRateManager::default();
        m.set_tick_rate(f32::NAN);
        assert!(m.tickrate().is_nan(), "Math.max(NaN, 1.0f) is NaN, not 1.0");
        // `(long)(1e9 / NaN)` is 0 in Java, and `as i64` maps NaN to 0 too.
        assert_eq!(m.nanoseconds_per_tick(), 0);
    }

    /// Rule 2. The tick length is derived from the **clamped** rate.
    ///
    /// A rate of 0.25 clamps to 1.0, so the tick is one second — not the four
    /// seconds the raw rate implies. The two readings differ by 4×.
    ///
    /// MUTATION: computing `nanoseconds_per_tick` from the argument rather
    /// than from `self.tickrate` after the clamp.
    #[test]
    fn the_tick_length_comes_from_the_clamped_rate_not_the_raw_one() {
        let mut m = TickRateManager::default();
        assert_eq!(m.nanoseconds_per_tick(), 50_000_000, "20 Hz is 50 ms");
        m.set_tick_rate(0.25);
        assert_eq!(m.nanoseconds_per_tick(), NANOSECONDS_PER_SECOND);
        assert_ne!(m.nanoseconds_per_tick(), 4 * NANOSECONDS_PER_SECOND);
        m.set_tick_rate(10.0);
        assert_eq!(m.nanoseconds_per_tick(), 100_000_000);
        assert_eq!(m.milliseconds_per_tick(), 100.0);
    }

    /// An unfrozen world runs every tick, and freezing stops it.
    ///
    /// MUTATION: inverting `is_frozen` in `tick`'s expression. Caught here
    /// rather than in the step witness, because with steps queued both
    /// readings agree.
    #[test]
    fn freezing_stops_game_elements_and_unfreezing_resumes_them() {
        let mut m = TickRateManager::default();
        m.tick();
        assert!(m.runs_normally());
        m.set_frozen(true);
        m.tick();
        assert!(!m.runs_normally());
        m.set_frozen(false);
        m.tick();
        assert!(m.runs_normally());
    }

    /// Rule 3. `/tick step 1` on a frozen world runs **exactly one** tick.
    ///
    /// This is the sample that sits where the mutation bites: with a step
    /// count of 1, reading the counter after the decrement gives `0 > 0` —
    /// false — so the stepped tick never runs and the command does nothing.
    /// A step count of 2 would let both readings run at least one tick and
    /// hide it.
    ///
    /// MUTATION: decrementing `frozen_ticks_to_run` before computing
    /// `run_game_elements`.
    #[test]
    fn a_single_step_runs_exactly_one_tick_on_a_frozen_world() {
        let mut m = TickRateManager::default();
        m.set_frozen(true);
        m.set_frozen_ticks_to_run(1);
        assert!(m.is_stepping_forward());

        m.tick();
        assert!(m.runs_normally(), "the stepped tick must actually run");
        assert_eq!(m.frozen_ticks_to_run(), 0);

        m.tick();
        assert!(!m.runs_normally(), "and only one");
    }

    /// A multi-step burns down one per tick and then re-freezes.
    ///
    /// MUTATION: decrementing by more than one, or clearing the counter
    /// outright — both leave the single-step witness above green.
    #[test]
    fn a_multi_step_burns_exactly_one_tick_at_a_time() {
        let mut m = TickRateManager::default();
        m.set_frozen(true);
        m.set_frozen_ticks_to_run(3);
        for expect in [2, 1, 0] {
            m.tick();
            assert!(m.runs_normally());
            assert_eq!(m.frozen_ticks_to_run(), expect);
        }
        m.tick();
        assert!(!m.runs_normally());
    }

    /// `setFrozenTicksToRun` assigns rather than accumulates.
    ///
    /// MUTATION: `+=`. Two `/tick step 5` would then queue ten, and a client
    /// that ran twice as long as the operator asked would look like a
    /// desynchronised world rather than a decode bug.
    #[test]
    fn setting_the_step_count_replaces_it() {
        let mut m = TickRateManager::default();
        m.set_frozen_ticks_to_run(5);
        m.set_frozen_ticks_to_run(5);
        assert_eq!(m.frozen_ticks_to_run(), 5);
    }

    /// The counter is only meaningful while frozen, but it is not *gated* on
    /// frozen: vanilla decrements it either way, so a step queued before an
    /// unfreeze drains harmlessly instead of lying in wait.
    ///
    /// MUTATION: guarding the decrement on `is_frozen`.
    #[test]
    fn queued_steps_drain_even_when_not_frozen() {
        let mut m = TickRateManager::default();
        m.set_frozen_ticks_to_run(2);
        m.tick();
        assert_eq!(m.frozen_ticks_to_run(), 1);
        m.tick();
        assert_eq!(m.frozen_ticks_to_run(), 0);
    }

    /// The id is the whole discriminator. Both bodies are decodable as the
    /// other's shape is not — but a `ticking_step` body of one byte would
    /// simply fail to read as an f32, so the observable failure of a swap is
    /// silence, not a wrong number.
    ///
    /// MUTATION: swapping the two arms of `kind_for_id`.
    #[test]
    fn each_ticking_id_maps_to_its_own_packet() {
        let ids = TickingIds {
            state: 127,
            step: 128,
        };
        assert_eq!(kind_for_id(127, ids), Some(TickingPacket::State));
        assert_eq!(kind_for_id(128, ids), Some(TickingPacket::Step));
        assert_eq!(kind_for_id(0, ids), None);
    }

    /// Through the routing seam: `ticking_state` writes **both** the rate and
    /// the freeze flag, and `ticking_step` writes the counter.
    ///
    /// The sample freezes and slows in one packet, so dropping either
    /// assignment is visible. MUTATION: `apply` calling only `set_tick_rate`
    /// — `handleTickingState` calls both, and a client that learned the rate
    /// but not the freeze would run a frozen world at full speed.
    #[test]
    fn routing_a_ticking_state_applies_the_rate_and_the_freeze() {
        let mut m = TickRateManager::default();
        let mut body = 5.0f32.to_be_bytes().to_vec();
        body.push(0x01);
        assert!(apply(TickingPacket::State, &body, &mut m));
        assert_eq!(m.tickrate(), 5.0);
        assert!(m.is_frozen());
        assert_eq!(m.nanoseconds_per_tick(), 200_000_000);

        assert!(apply(TickingPacket::Step, &[0x03], &mut m));
        assert_eq!(m.frozen_ticks_to_run(), 3);
    }

    /// A malformed body is reported rather than half-applied.
    ///
    /// MUTATION: `apply` returning `true` on a decode error. Note the state
    /// really is untouched here — `read_ticking_state` reads the whole body
    /// before anything is assigned, which is why the reader returns a tuple
    /// instead of writing as it goes.
    #[test]
    fn a_malformed_ticking_body_reports_false_and_changes_nothing() {
        let mut m = TickRateManager::default();
        let before = m;
        assert!(!apply(TickingPacket::State, &[0x00], &mut m));
        assert!(!apply(TickingPacket::Step, &[], &mut m));
        assert_eq!(m, before);
    }

    /// The pre-server defaults are vanilla's field initialisers, so a session
    /// that never receives a `ticking_state` still reads 20 Hz / running —
    /// which is precisely the assumption Rewo has been making implicitly.
    ///
    /// MUTATION: `Default::default()` deriving zeros. A 0 Hz default divides
    /// to an infinite tick length.
    #[test]
    fn the_defaults_are_twenty_hertz_and_running() {
        let m = TickRateManager::default();
        assert_eq!(m.tickrate(), 20.0);
        assert_eq!(m.nanoseconds_per_tick(), 50_000_000);
        assert!(!m.is_frozen());
        assert!(m.runs_normally());
        assert!(!m.is_stepping_forward());
    }
}
