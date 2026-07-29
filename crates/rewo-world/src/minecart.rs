//! `NewMinecartBehavior`'s **client half** — the interpolation schedule
//! `move_minecart_along_track` fills (M77).
//!
//! ## Why a minecart does not use the generic 3-tick lerp
//!
//! Every other entity in this client moves through
//! [`crate::entities::EntityState`]'s model: a packet writes an authoritative
//! target and the rendered position walks a third of the remaining distance
//! per tick. A minecart on the experimental movement path does not, and the
//! reason is structural on **both** sides of the wire:
//!
//! * *Server.* `ServerEntity.sendChanges` has an `else if` for
//!   `entity instanceof AbstractMinecart && getBehavior() instanceof
//!   NewMinecartBehavior` that **replaces** the whole generic position branch
//!   with `handleMinecartPosRot`. Such a cart is therefore never sent
//!   `move_entity_pos`, `teleport_entity` or `entity_position_sync` at all —
//!   `move_minecart_along_track` is its only movement channel. (And when the
//!   cart is at rest the same method still sends one, carrying a single
//!   `weight 1.0` step at the current position.)
//! * *Client.* `AbstractMinecart.getInterpolation()` forwards to
//!   `behavior.getInterpolation()`, and `NewMinecartBehavior` does not
//!   override `MinecartBehavior`'s `return null`. So a stray positional packet
//!   would take `Entity.moveOrInterpolateTo`'s null branch and **snap**, not
//!   interpolate. (`OldMinecartBehavior` is the half that owns a real
//!   `InterpolationHandler`.)
//!
//! ## But it does not replace the generic lerp either — both run
//!
//! The schedule writes the entity's position once per tick, at its own
//! `partialTicks = 1.0`. That write is an ordinary `Entity.setPos`, so
//! `xOld → getX()` keeps tracking it and `EntityRenderer.extractRenderState`'s
//! `Mth.lerp(partialTicks, xOld, getX())` still produces a position — the
//! tick-quantised chord between two consecutive schedule samples.
//! `AbstractMinecartRenderer.newExtractState` then **overrides** it with
//! `getCartLerpPosition(partialTicks)`, the schedule sampled at the true
//! partial tick, but only `if (behavior.cartHasPosRotLerp())`.
//!
//! The two coexist and vanilla measures one against the other: a passenger of
//! a lerping cart gets `state.passengerOffset =
//! getCartLerpPosition(partialTicks) - lerp(partialTicks, xOld, getX())` —
//! literally the difference between the schedule and the generic lerp. They
//! agree exactly at `partialTicks == 1.0` (that is the sample the tick wrote)
//! and diverge in between whenever the segment crosses a step boundary within
//! one tick, because the schedule's alpha is piecewise-affine over a weighted
//! step list where the generic chord is one straight line.
//!
//! So the answer to "does this replace the existing interpolation or feed it"
//! is neither: it **overrides it at the render seam and leaves it running**.
//! That is the mirror image of M72's passenger finding, where a rider's own
//! lerp is computed and thrown away by `positionRider` — here the generic lerp
//! is computed, kept, and is the baseline the finer sample is measured
//! against.
//!
//! ## What is deliberately not modelled
//!
//! * `getCurrentLerpStep`'s `cacheIndexAlpha` memo, keyed on
//!   `(partialTick, lerpDelay)`. It is a pure memo — vanilla calls the method
//!   four times per sample and gets one answer; [`MinecartLerp::sample`]
//!   computes it once and uses it four times, which is the same answer.
//! * The whole server-side half of `NewMinecartBehavior` (`moveAlongTrack`,
//!   `adjustToRails`, the rail-shape speed maths). A client never runs it; the
//!   steps it produces arrive on the wire already computed.

/// `NewMinecartBehavior.POS_ROT_LERP_TICKS`. Also spelled as the literal `3`
/// inside `getCurrentLerpStep`'s alpha, which is transcribed here as the same
/// constant on the assumption they are the same 3 — they are, and vanilla
/// writing one of them as a literal is why this comment exists.
pub const POS_ROT_LERP_TICKS: i32 = 3;

/// `NewMinecartBehavior.MinecartStep` — one scheduled sample.
///
/// Wire form (`MinecartStep.STREAM_CODEC`): `Vec3.STREAM_CODEC` twice — **full
/// doubles**, not the `LP_STREAM_CODEC` packing `set_entity_motion` uses —
/// then two `ByteBufCodecs.ROTATION_BYTE` and one big-endian `f32`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MinecartStep {
    pub position: [f64; 3],
    pub movement: [f64; 3],
    pub y_rot: f32,
    pub x_rot: f32,
    /// How much of the segment this step spans. A **zero** weight is not
    /// "no movement": `adjustToRails(…, instant = true)` emits one, and a
    /// segment whose weights sum to zero is snapped to rather than traversed.
    pub weight: f32,
}

impl MinecartStep {
    /// `NewMinecartBehavior.MinecartStep.ZERO` — also the initial `oldLerp`.
    pub const ZERO: Self = Self {
        position: [0.0; 3],
        movement: [0.0; 3],
        y_rot: 0.0,
        x_rot: 0.0,
        weight: 0.0,
    };
}

/// What the schedule resolves to at one partial tick: the four values
/// `lerpClientPositionAndRotation` assigns, and the four
/// `AbstractMinecartRenderer` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinecartSample {
    pub position: [f64; 3],
    pub movement: [f64; 3],
    pub y_rot: f32,
    pub x_rot: f32,
}

/// `NewMinecartBehavior`'s client state: the inbox the packet appends to, the
/// segment currently being traversed, and the countdown between them.
#[derive(Clone, Debug, Default)]
pub struct MinecartLerp {
    /// `lerpSteps` — what arriving packets append to. Drained wholesale when
    /// the countdown expires.
    inbox: Vec<MinecartStep>,
    /// `currentLerpSteps` — the segment being traversed right now.
    current: Vec<MinecartStep>,
    /// `currentLerpStepsTotalWeight`. A `double` in vanilla even though every
    /// weight is a `float`, which changes the arithmetic in
    /// [`Self::current_step`] and is transcribed rather than tidied.
    total_weight: f64,
    /// `oldLerp` — the position the segment starts from, snapshotted off the
    /// live entity at ingest.
    old: MinecartStep,
    /// `lerpDelay`. Pre-decremented every tick, so it runs negative while
    /// nothing is arriving; only `<= 0` is ever tested.
    delay: i32,
    /// The entity's `deltaMovement`. Vanilla keeps it on the `Entity`; Rewo's
    /// [`crate::entities::EntityState`] has no velocity field, and for a cart
    /// on this path the schedule is the only writer — `handleMinecartPosRot`
    /// never sends one a `set_entity_motion`. So it lives here, and
    /// `setOldLerpValues` reads it from here.
    movement: [f64; 3],
}

impl MinecartLerp {
    /// `handleMinecartAlongTrack`: `newMinecartBehavior.lerpSteps.addAll(...)`.
    ///
    /// An **append**, never a replace. Two packets that arrive between two
    /// client ticks are one segment, which is why the server is free to send a
    /// multi-step packet per tick and the client is free to miss a tick.
    pub fn push_steps(&mut self, steps: &[MinecartStep]) {
        self.inbox.extend_from_slice(steps);
    }

    /// `cartHasPosRotLerp()`.
    pub fn has_lerp(&self) -> bool {
        !self.current.is_empty()
    }

    /// `lerpClientPositionAndRotation()`, minus the four assignments — the
    /// caller applies the returned sample to the entity.
    ///
    /// `pos` / `y_rot` / `x_rot` are the entity's live values, which
    /// `setOldLerpValues()` snapshots when a new segment starts.
    #[must_use]
    pub fn tick(&mut self, pos: [f64; 3], y_rot: f32, x_rot: f32) -> Option<MinecartSample> {
        // `if (--this.lerpDelay <= 0)`. Saturating rather than wrapping: Java's
        // int wraps at `i32::MIN`, which needs ~3.4 years of continuous ticks
        // on one cart with nothing ever arriving, and a debug-build Rust panic
        // there would be a worse answer than a stuck floor.
        self.delay = self.delay.saturating_sub(1);
        if self.delay <= 0 {
            // `setOldLerpValues()` — weight 0, so the old value is never itself
            // a scheduled stop, only the origin the first step is measured from.
            self.old = MinecartStep {
                position: pos,
                movement: self.movement,
                y_rot,
                x_rot,
                weight: 0.0,
            };
            self.current.clear();
            if !self.inbox.is_empty() {
                self.current.append(&mut self.inbox);
                // Reset *inside* this branch, as vanilla does. With an empty
                // inbox the stale total survives — unread, because
                // `cartHasPosRotLerp()` is false the moment `current` is empty.
                self.total_weight = 0.0;
                for step in &self.current {
                    self.total_weight += step.weight as f64;
                }
                // A zero-weight segment is a snap, not a traversal: the delay
                // stays 0 so the next tick immediately ingests again.
                self.delay = if self.total_weight == 0.0 {
                    0
                } else {
                    POS_ROT_LERP_TICKS
                };
            }
        }
        let sample = self.sample(1.0)?;
        // `setDeltaMovement(getCartLerpMovements(1.0F))`.
        self.movement = sample.movement;
        Some(sample)
    }

    /// `getCartLerp{Position,Movements,XRot,YRot}(partialTicks)` — all four,
    /// off one `getCurrentLerpStep`.
    ///
    /// `None` exactly when `cartHasPosRotLerp()` is false. Vanilla has no such
    /// branch because every call site is already inside that guard; an
    /// unguarded call there indexes `currentLerpSteps.get(-1)` and throws.
    pub fn sample(&self, partial: f32) -> Option<MinecartSample> {
        let (t, current, previous) = self.current_step(partial)?;
        Some(MinecartSample {
            position: lerp3(t, previous.position, current.position),
            movement: lerp3(t, previous.movement, current.movement),
            y_rot: rot_lerp(t, previous.y_rot, current.y_rot),
            x_rot: rot_lerp(t, previous.x_rot, current.x_rot),
        })
    }

    /// `getCurrentLerpStep(partialTick)` → `(partialTicksInStep, currentStep,
    /// previousStep)`.
    ///
    /// The arithmetic mixes widths on purpose. `alpha` is a `float`,
    /// `total_weight` is a `double`, and `countUp` is a `float` — so the
    /// threshold comparison and the in-step fraction are both evaluated in
    /// `f64` with the floats widened, and only the fraction is narrowed back.
    /// Doing it all in `f32`, or all in `f64`, both drift from vanilla.
    fn current_step(&self, partial: f32) -> Option<(f32, MinecartStep, MinecartStep)> {
        if self.current.is_empty() {
            return None;
        }
        // `(3 - this.lerpDelay + partialTick) / 3.0F` — the `3 - lerpDelay` is
        // integer arithmetic, and the divisor is the same 3.
        let alpha = ((POS_ROT_LERP_TICKS - self.delay) as f32 + partial) / POS_ROT_LERP_TICKS as f32;
        let mut count_up = 0.0f32;
        let mut indexed_partial = 1.0f32;
        let mut found = false;
        let mut index = 0usize;
        for (i, step) in self.current.iter().enumerate() {
            let weight = step.weight;
            // `if (!(weight <= 0.0F))` — a zero-weight step is skipped for the
            // purpose of *finding* the index, but is still selectable as the
            // fallback below, which is how an instant `adjustToRails` step is
            // snapped to.
            if !(weight <= 0.0) {
                count_up += weight;
                if count_up as f64 >= self.total_weight * alpha as f64 {
                    let current = count_up - weight;
                    indexed_partial =
                        ((alpha as f64 * self.total_weight - current as f64) / weight as f64) as f32;
                    found = true;
                    index = i;
                    break;
                }
            }
        }
        if !found {
            index = self.current.len() - 1;
        }
        let current = self.current[index];
        // `index > 0 ? currentLerpSteps.get(index - 1) : this.oldLerp`.
        let previous = if index > 0 {
            self.current[index - 1]
        } else {
            self.old
        };
        Some((indexed_partial, current, previous))
    }

    /// How many steps the current segment holds — for witnesses, so a gate can
    /// tell "the segment expired" from "the segment is still running".
    pub fn segment_len(&self) -> usize {
        self.current.len()
    }

    /// How many steps are queued for the next segment.
    pub fn inbox_len(&self) -> usize {
        self.inbox.len()
    }

    /// `lerpDelay`.
    pub fn delay(&self) -> i32 {
        self.delay
    }

    /// `currentLerpStepsTotalWeight`.
    pub fn total_weight(&self) -> f64 {
        self.total_weight
    }
}

/// `Mth.lerp(double alpha, Vec3 p1, Vec3 p2)`. The `float partialTicksInStep`
/// widens to `double` at the call, so the whole blend runs in `f64`.
fn lerp3(alpha: f32, from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
    let a = alpha as f64;
    [
        from[0] + a * (to[0] - from[0]),
        from[1] + a * (to[1] - from[1]),
        from[2] + a * (to[2] - from[2]),
    ]
}

/// `Mth.rotLerp(float a, float from, float to)` = `from + a * wrapDegrees(to -
/// from)`, all in `f32`.
fn rot_lerp(a: f32, from: f32, to: f32) -> f32 {
    from + a * wrap_degrees(to - from)
}

/// `Mth.wrapDegrees(float)`. Rust's `%` on floats is Java's `%`: a truncated
/// remainder carrying the dividend's sign, so the two guards below cover the
/// same cases they do there.
fn wrap_degrees(angle: f32) -> f32 {
    let mut n = angle % 360.0;
    if n >= 180.0 {
        n -= 360.0;
    }
    if n < -180.0 {
        n += 360.0;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(x: f64, z: f64, weight: f32) -> MinecartStep {
        MinecartStep {
            position: [x, 0.0, z],
            movement: [0.0; 3],
            y_rot: 0.0,
            x_rot: 0.0,
            weight,
        }
    }

    #[test]
    fn the_zero_step_is_the_default() {
        assert_eq!(MinecartStep::default(), MinecartStep::ZERO);
    }

    #[test]
    fn an_empty_schedule_samples_to_nothing() {
        let mut l = MinecartLerp::default();
        assert!(!l.has_lerp());
        assert_eq!(l.sample(1.0), None);
        // Ticking an empty schedule is a no-op that still burns the delay,
        // exactly as vanilla's unconditional pre-decrement does.
        assert_eq!(l.tick([1.0, 2.0, 3.0], 0.0, 0.0), None);
        assert_eq!(l.delay(), -1);
    }

    #[test]
    fn one_step_is_traversed_over_exactly_three_ticks() {
        let mut l = MinecartLerp::default();
        l.push_steps(&[step(30.0, 0.0, 1.0)]);
        let mut pos = [0.0, 0.0, 0.0];
        let mut seen = Vec::new();
        for _ in 0..3 {
            let s = l.tick(pos, 0.0, 0.0).expect("segment running");
            pos = s.position;
            seen.push(s.position[0]);
        }
        // alpha = 1/3, 2/3, 1 -> 10, 20, 30 from the ingest-time origin. The
        // tolerance is 1e-5 rather than exact because `alpha` is a **float** in
        // vanilla and here: 1/3 as `f32` is 0.33333334, so the first two stops
        // land ~3e-7 past their exact thirds. Tightening this below the f32
        // step would be asserting more than the transcription claims.
        for (got, want) in seen.iter().zip([10.0, 20.0, 30.0]) {
            assert!((got - want).abs() < 1e-5, "{seen:?}");
        }
        // A fourth tick expires the segment: the inbox is empty, so
        // `currentLerpSteps` is cleared and nothing is written.
        assert_eq!(l.tick(pos, 0.0, 0.0), None);
        assert!(!l.has_lerp());
    }

    #[test]
    fn steps_accumulate_rather_than_replace() {
        let mut l = MinecartLerp::default();
        l.push_steps(&[step(1.0, 0.0, 1.0)]);
        l.push_steps(&[step(2.0, 0.0, 1.0)]);
        assert_eq!(l.inbox_len(), 2);
        l.tick([0.0; 3], 0.0, 0.0).expect("ingested");
        assert_eq!(l.segment_len(), 2);
        assert_eq!(l.inbox_len(), 0);
        assert!((l.total_weight() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_zero_weight_segment_snaps_and_re_ingests_immediately() {
        let mut l = MinecartLerp::default();
        l.push_steps(&[step(5.0, 0.0, 0.0)]);
        let s = l.tick([0.0; 3], 0.0, 0.0).expect("ingested");
        // No weighted step is findable, so the fallback selects the last one at
        // `indexedPartialTick = 1.0` — a snap, not a third of the way.
        assert!((s.position[0] - 5.0).abs() < 1e-9, "{s:?}");
        assert_eq!(l.delay(), 0);
    }

    #[test]
    fn rot_lerp_takes_the_short_way_round() {
        // 350 -> 10 is +20, not -340.
        assert!((rot_lerp(0.5, 350.0, 10.0) - 360.0).abs() < 1e-4);
        assert!((wrap_degrees(-190.0) - 170.0).abs() < 1e-4);
        // The bound is `>= 180`, so exactly 180 wraps to -180.
        assert!((wrap_degrees(180.0) + 180.0).abs() < 1e-4);
    }
}
