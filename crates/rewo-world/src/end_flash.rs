//! `EndFlashState` — the End's periodic sky flash.
//!
//! Transcribed from `net/minecraft/client/renderer/EndFlashState.java`. One
//! `long` in, everything out: the flash schedule is a pure function of the
//! dimension's clock, so this module has no packet, no camera and no player
//! input, and every value it produces is independently computable by a test.
//!
//! It has three consumers in vanilla, which is why it lives in `rewo-world`
//! rather than beside any one of them:
//!
//! * `SkyRenderer.java:270-274` — the flash quad's intensity and bearing.
//! * `LightmapRenderStateExtractor.java:57-65` — `skyFactor += intensity`,
//!   so the world itself brightens.
//! * `ClientLevel.java:307-324` — a `DirectionalSoundInstance` queued 30
//!   ticks behind the visible flash.
//!
//! # The clock is the dimension's own, not the overworld's
//!
//! `ClientLevel.java:308` ticks this with `getDefaultClockTime()`, which is
//! `Level.java:889-891` → `clockManager().getTotalTicks(dimensionType()
//! .defaultClock())`. `data/minecraft/dimension_type/the_end.json` declares
//! `"default_clock": "minecraft:the_end"`, a different clock from the
//! overworld's — a vanilla server sends both, and reading the overworld's
//! would put the flash at the wrong phase.
//!
//! # Four transcription details, each of which inverts if guessed
//!
//! **1. The first interval never flashes.** `private long flashSeed;`
//! (`EndFlashState.java:12`) has no initializer, so Java defaults it to `0`,
//! and `calculateFlashParameters` draws only when `newSeed != flashSeed`. For
//! `clock_time` in `0..600` the new seed *is* `0`, so the guard holds and
//! `offset`/`duration`/`x_angle`/`y_angle` stay at their own zero defaults —
//! which makes the flash window `[0, 0]` and the intensity zero throughout.
//! Priming the parameters in a constructor, or modelling the seed as
//! "not yet computed", produces a flash in the first 600 ticks that vanilla
//! does not have. [`EndFlashState::default`] reproduces Java's field defaults
//! deliberately; see `first_interval_never_flashes`.
//!
//! **2. `Mth.sin` is what keeps tick 0 finite.** Inside that first interval,
//! `clock_time == 0` satisfies `within >= offset && within <= offset +
//! duration` (both sides `0`), so the expression evaluates
//! `0.0 * PI / 0` — `NaN`. It comes out as `0.0` only because `Mth.sin`
//! (`Mth.java:50-52`) is a 65536-entry table lookup whose `(long)(NaN *
//! SIN_SCALE)` narrows to `0` and whose `SIN[0]` is `0.0`. Platform `sin`
//! would propagate the `NaN` into the lightmap and the vertex colour.
//! [`crate::lightmap::mth_sin`] is the port, and its cast reproduces the
//! narrowing.
//!
//! **3. `Mth.randomBetweenInclusive` always draws.** `Mth.java:690-692` has
//! no `min >= max` guard, unlike `Mth.nextInt` twelve lines earlier at
//! `Mth.java:146-148`, which returns `min` without touching the source. The
//! two read as interchangeable and are not; getting it wrong here would
//! desynchronise every draw after it.
//!
//! **4. One draw is discarded.** `EndFlashState.java:30` calls
//! `randomSource.nextFloat()` and throws the result away before the four real
//! draws. Skipping it shifts the whole sequence, so every offset, duration
//! and angle is wrong, with nothing anywhere reporting an error.
//!
//! # `Math.min(380, 600 - offset)` is inert
//!
//! `EndFlashState.java:32` clamps the duration's upper bound so a flash
//! cannot outlive its interval. With the shipped constants it never bites:
//! `offset` is drawn from `[0, 200]`, so `600 - offset` is at least `400`,
//! and `min(380, ..)` is `380` for every reachable offset. It is transcribed
//! rather than folded away, and `duration_clamp_is_inert` proves the
//! inertness exhaustively so a future constant change makes the branch live
//! again instead of silently doing nothing. The same arithmetic is why
//! `offset + duration <= 580 < 600`: a flash always completes inside the
//! interval that scheduled it, which is what stops the parameters ever
//! changing mid-flash.
//!
//! # The RNG
//!
//! `RandomSource.createThreadLocalInstance(seed)` (`RandomSource.java:31-33`)
//! builds a `SingleThreadedRandomSource`, which is `BitRandomSource` with
//! multiplier `25214903917`, increment `11` and a 48-bit mask — i.e. exactly
//! `java.util.Random`, and exactly [`crate::particles::LegacyRandom`], which
//! is already graded bit-for-bit against a JVM. The only difference from
//! `LegacyRandomSource` is that the seed is a plain field rather than an
//! `AtomicLong`, which no single-threaded caller can observe.

use crate::lightmap::mth_sin;
use crate::particles::LegacyRandom;

/// `EndFlashState.FLASH_INTERVAL_IN_TICKS`.
pub const FLASH_INTERVAL_IN_TICKS: i64 = 600;
/// `EndFlashState.MAX_FLASH_OFFSET_IN_TICKS`.
pub const MAX_FLASH_OFFSET_IN_TICKS: i32 = 200;
/// `EndFlashState.MIN_FLASH_DURATION_IN_TICKS`.
pub const MIN_FLASH_DURATION_IN_TICKS: i32 = 100;
/// `EndFlashState.MAX_FLASH_DURATION_IN_TICKS`.
pub const MAX_FLASH_DURATION_IN_TICKS: i32 = 380;
/// `EndFlashState.SOUND_DELAY_IN_TICKS` — the flash's sound is queued this
/// many ticks behind the frame the flash starts on.
pub const SOUND_DELAY_IN_TICKS: i32 = 30;

/// The End's flash schedule for one dimension's clock.
///
/// [`Default`] reproduces Java's field defaults exactly, including
/// `flash_seed == 0`; see the module doc's detail 1 for why that is
/// load-bearing rather than incidental.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EndFlashState {
    flash_seed: i64,
    offset: i32,
    duration: i32,
    intensity: f32,
    old_intensity: f32,
    x_angle: f32,
    y_angle: f32,
}

impl EndFlashState {
    /// `EndFlashState.tick(long)`.
    ///
    /// The parameter recalculation runs **before** `old_intensity` is
    /// captured, matching `EndFlashState.java:20-24`. That ordering is only
    /// unobservable because a flash always ends before its interval does (see
    /// the module doc's inertness note); it is kept anyway rather than
    /// reordered into something that looks tidier.
    pub fn tick(&mut self, clock_time: i64) {
        self.calculate_flash_parameters(clock_time);
        self.old_intensity = self.intensity;
        self.intensity = self.calculate_intensity(clock_time);
    }

    /// `EndFlashState.calculateFlashParameters`.
    ///
    /// `clockTime / 600L` is Java `long` division, which truncates toward
    /// zero; Rust's `/` does the same, so a negative clock — reachable only
    /// through a wrapped `totalTicks` — lands on the same seed. `div_euclid`
    /// would not.
    fn calculate_flash_parameters(&mut self, clock_time: i64) {
        let new_seed = clock_time / FLASH_INTERVAL_IN_TICKS;
        if new_seed == self.flash_seed {
            return;
        }
        let mut random = LegacyRandom::new(new_seed);
        // `EndFlashState.java:30` — discarded, and load-bearing.
        let _ = random.next_float();
        self.offset = random_between_inclusive(&mut random, 0, MAX_FLASH_OFFSET_IN_TICKS);
        self.duration = random_between_inclusive(
            &mut random,
            MIN_FLASH_DURATION_IN_TICKS,
            MAX_FLASH_DURATION_IN_TICKS
                .min(FLASH_INTERVAL_IN_TICKS as i32 - self.offset),
        );
        self.x_angle = random_between(&mut random, -60.0, 10.0);
        self.y_angle = random_between(&mut random, -180.0, 180.0);
        self.flash_seed = new_seed;
    }

    /// `EndFlashState.calculateIntensity`.
    ///
    /// The float expression is `(t * PI) / duration`, left to right, and not
    /// `t * (PI / duration)` — same-precedence operators associate leftward
    /// in Java, and the two round differently.
    fn calculate_intensity(&self, clock_time: i64) -> f32 {
        let within = clock_time % FLASH_INTERVAL_IN_TICKS;
        if within >= i64::from(self.offset)
            && within <= i64::from(self.offset) + i64::from(self.duration)
        {
            mth_sin(
                (within - i64::from(self.offset)) as f32 * std::f32::consts::PI
                    / self.duration as f32,
            )
        } else {
            0.0
        }
    }

    /// `EndFlashState.getXAngle` — the flash's pitch, drawn from
    /// `[-60, 10)` degrees, so it is almost always above the horizon.
    pub fn x_angle(&self) -> f32 {
        self.x_angle
    }

    /// `EndFlashState.getYAngle` — the flash's yaw, drawn from
    /// `[-180, 180)` degrees.
    pub fn y_angle(&self) -> f32 {
        self.y_angle
    }

    /// `EndFlashState.getIntensity(float)` — `Mth.lerp(partialTicks,
    /// oldIntensity, intensity)`, i.e. `old + partial * (new - old)`
    /// (`Mth.java:550-552`).
    pub fn intensity(&self, partial_ticks: f32) -> f32 {
        self.old_intensity + partial_ticks * (self.intensity - self.old_intensity)
    }

    /// `EndFlashState.flashStartedThisTick` — the rising edge the delayed
    /// sound hangs off. Note the asymmetry: `> 0.0` on the new value and
    /// `<= 0.0` on the old.
    pub fn flash_started_this_tick(&self) -> bool {
        self.intensity > 0.0 && self.old_intensity <= 0.0
    }
}

/// `Mth.randomBetweenInclusive` (`Mth.java:690-692`) — `nextInt(max - min + 1)
/// + min`, with **no** `min >= max` guard. Its neighbour `Mth.nextInt`
/// (`Mth.java:146-148`) has one and does not draw; they are not
/// interchangeable.
fn random_between_inclusive(random: &mut LegacyRandom, min: i32, max_inclusive: i32) -> i32 {
    random.next_int(max_inclusive - min + 1) + min
}

/// `Mth.randomBetween` (`Mth.java:694-696`) — `nextFloat() * (max - min) +
/// min`, exclusive at the top.
fn random_between(random: &mut LegacyRandom, min: f32, max_exclusive: f32) -> f32 {
    random.next_float() * (max_exclusive - min) + min
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An independent transliteration of `BitRandomSource`, `nextInt` and
    /// `nextFloat`, written from `BitRandomSource.java` and
    /// `SingleThreadedRandomSource.java` rather than from
    /// [`crate::particles::LegacyRandom`], so a witness driving it is not
    /// asserting that the production RNG equals itself.
    ///
    /// The primitives are anchored to a real JVM elsewhere —
    /// `particles`'s `next_float_matches_jvm_bit_for_bit` and
    /// `next_int_matches_jvm_bit_for_bit`. What this anchors is the
    /// *composition*: the discarded draw, the order of the four real draws,
    /// and each one's bounds. [`VECTORS`] closes the loop with literals from
    /// a third implementation, so the pair cannot drift together.
    struct Oracle(u64);

    impl Oracle {
        fn new(seed: i64) -> Self {
            Oracle(((seed as u64) ^ 25_214_903_917) & ((1 << 48) - 1))
        }

        fn next(&mut self, bits: u32) -> i32 {
            self.0 = self.0.wrapping_mul(25_214_903_917).wrapping_add(11) & ((1 << 48) - 1);
            (self.0 >> (48 - bits)) as i32
        }

        fn next_int(&mut self, bound: i32) -> i32 {
            if bound & (bound - 1) == 0 {
                return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
            }
            loop {
                let sample = self.next(31);
                let modulo = sample % bound;
                if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                    return modulo;
                }
            }
        }

        fn next_float(&mut self) -> f32 {
            self.next(24) as f32 * 5.960_464_5e-8
        }
    }

    /// The four parameters for one interval seed, derived independently.
    /// `discard` exists so a witness can ask what the schedule would be
    /// *without* `EndFlashState.java:30`'s thrown-away draw.
    fn oracle_params(seed: i64, discard: bool) -> (i32, i32, f32, f32) {
        let mut r = Oracle::new(seed);
        if discard {
            r.next_float();
        }
        let offset = r.next_int(200 - 0 + 1) + 0;
        let duration = r.next_int(380.min(600 - offset) - 100 + 1) + 100;
        let x_angle = r.next_float() * (10.0 - -60.0) + -60.0;
        let y_angle = r.next_float() * (180.0 - -180.0) + -180.0;
        (offset, duration, x_angle, y_angle)
    }

    /// Tick a fresh state forward one tick at a time from 0.
    ///
    /// The state machine is a *sequence*, not a function of the final tick —
    /// `old_intensity` is last tick's value — so a witness that jumps
    /// straight to one tick is measuring a state vanilla never reaches.
    fn ticked_to(clock_time: i64) -> EndFlashState {
        let mut s = EndFlashState::default();
        for t in 0..=clock_time {
            s.tick(t);
        }
        s
    }

    // ---- the schedule -----------------------------------------------------

    /// Literal vectors from a third implementation of the same LCG, held as
    /// literals so [`Oracle`] and the production path cannot drift together
    /// into a shared misreading.
    const VECTORS: &[(i64, i32, i32, f32, f32)] = &[
        (1, 133, 311, -31.479_216, -105.222_671),
        (2, 138, 296, -59.709_076, -1.143_890_4),
        (3, 86, 289, -3.632_457_7, -155.836_822),
        (7, 29, 334, -59.325_974, -54.608_513),
        (12345, 151, 174, 4.198_028_6, 119.912_872),
    ];

    /// The five constants, against their source rather than against
    /// themselves.
    ///
    /// M93r's sweep exists for this: every *behavioural* witness in this
    /// module measures in units of [`FLASH_INTERVAL_IN_TICKS`] — the seed is
    /// `clock / interval` and the window is `clock % interval` — so changing
    /// the interval moves the expectation and the render together and the
    /// whole battery stays green. A mutation to `601` survived until this
    /// test existed. Pin a number against its source, not against itself.
    #[test]
    fn constants_match_the_decompile() {
        // `EndFlashState.java:7-11`.
        assert_eq!(SOUND_DELAY_IN_TICKS, 30);
        assert_eq!(FLASH_INTERVAL_IN_TICKS, 600);
        assert_eq!(MAX_FLASH_OFFSET_IN_TICKS, 200);
        assert_eq!(MIN_FLASH_DURATION_IN_TICKS, 100);
        assert_eq!(MAX_FLASH_DURATION_IN_TICKS, 380);
    }

    #[test]
    fn schedule_matches_literal_vectors() {
        for &(seed, offset, duration, x_angle, y_angle) in VECTORS {
            let s = ticked_to(seed * FLASH_INTERVAL_IN_TICKS);
            assert_eq!(s.offset, offset, "offset for seed {seed}");
            assert_eq!(s.duration, duration, "duration for seed {seed}");
            assert!(
                (s.x_angle() - x_angle).abs() < 1e-4,
                "x_angle for seed {seed}: {} vs {x_angle}",
                s.x_angle()
            );
            assert!(
                (s.y_angle() - y_angle).abs() < 1e-4,
                "y_angle for seed {seed}: {} vs {y_angle}",
                s.y_angle()
            );
        }
    }

    #[test]
    fn schedule_matches_independent_oracle() {
        for seed in 1..400i64 {
            let s = ticked_to(seed * FLASH_INTERVAL_IN_TICKS);
            let (offset, duration, x_angle, y_angle) = oracle_params(seed, true);
            assert_eq!((s.offset, s.duration), (offset, duration), "seed {seed}");
            assert_eq!(s.x_angle().to_bits(), x_angle.to_bits(), "x seed {seed}");
            assert_eq!(s.y_angle().to_bits(), y_angle.to_bits(), "y seed {seed}");
        }
    }

    /// The sensitivity partner for the two above: `EndFlashState.java:30`'s
    /// discarded `nextFloat()` moves every subsequent draw. Without this
    /// witness, an implementation *and* an oracle that both dropped the
    /// discard would agree with each other and be wrong together.
    #[test]
    fn discarded_draw_changes_the_schedule() {
        let differed = (1..400i64)
            .filter(|&seed| {
                let with = oracle_params(seed, true);
                let without = oracle_params(seed, false);
                (with.0, with.1) != (without.0, without.1)
            })
            .count();
        assert!(
            differed > 350,
            "the discard must move the schedule for essentially every seed; moved {differed}/399"
        );
        // And production takes the with-discard branch.
        let s = ticked_to(FLASH_INTERVAL_IN_TICKS);
        assert_eq!((s.offset, s.duration), (133, 311));
        assert_ne!(
            (s.offset, s.duration),
            (84, 243),
            "(84, 243) is seed 1's no-discard schedule"
        );
    }

    // ---- the first interval -----------------------------------------------

    /// `flashSeed` defaults to `0` and `calculateFlashParameters` draws only
    /// on a *change*, so the whole of interval 0 runs on zeroed parameters
    /// and never flashes. Exhaustive over the interval.
    ///
    /// The mutation this exists for is the natural "fix": priming the
    /// parameters at construction, or modelling the seed as an
    /// `Option<i64>` meaning "not yet computed". Either produces a flash in
    /// the first 600 ticks that vanilla does not have.
    #[test]
    fn first_interval_never_flashes() {
        let mut s = EndFlashState::default();
        for t in 0..FLASH_INTERVAL_IN_TICKS {
            s.tick(t);
            assert_eq!(s.intensity(1.0), 0.0, "tick {t}");
            assert!(!s.flash_started_this_tick(), "tick {t}");
        }
        assert_eq!((s.offset, s.duration), (0, 0));
        assert_eq!((s.x_angle(), s.y_angle()), (0.0, 0.0));
    }

    /// At `clock_time == 0` the window test `0 >= 0 && 0 <= 0 + 0` passes on
    /// zeroed parameters, so the intensity expression is `0.0 * PI / 0`,
    /// which is `NaN`. It comes out `0.0` only because `Mth.sin`
    /// (`Mth.java:50-52`) is a table lookup: `(long)(NaN * SIN_SCALE)`
    /// narrows to `0` and `SIN[0]` is `0.0`. Platform `sin` would propagate
    /// the `NaN` into the lightmap and the flash quad's vertex colour.
    #[test]
    fn tick_zero_is_finite_only_because_mth_sin_is_a_table() {
        let mut s = EndFlashState::default();
        s.tick(0);
        assert!(!s.intensity(1.0).is_nan(), "tick 0 must not be NaN");
        assert_eq!(s.intensity(1.0), 0.0);
        // The expression really does evaluate NaN; it is `mth_sin` that
        // rescues it. If this ever stops being NaN the witness above is
        // measuring something else.
        assert!((0.0f32 * std::f32::consts::PI / 0.0f32).is_nan());
        assert_eq!(crate::lightmap::mth_sin(f32::NAN), 0.0);
    }

    // ---- the inert clamp --------------------------------------------------

    /// `Math.min(380, 600 - offset)` (`EndFlashState.java:32`) exists so a
    /// flash cannot outlive its interval, and with the shipped constants it
    /// never bites: `offset` is drawn from `[0, 200]`, so `600 - offset` is
    /// at least `400`. Proved exhaustively rather than by inspection, so a
    /// future constant change makes the branch live instead of silently
    /// doing nothing. (`Mth.java:690`'s `centerY`-style inert-expression
    /// precedent: M104.)
    #[test]
    fn duration_clamp_is_inert() {
        for offset in 0..=MAX_FLASH_OFFSET_IN_TICKS {
            assert_eq!(
                MAX_FLASH_DURATION_IN_TICKS.min(FLASH_INTERVAL_IN_TICKS as i32 - offset),
                MAX_FLASH_DURATION_IN_TICKS,
                "offset {offset}"
            );
        }
    }

    /// The consequence of the clamp being inert *and* the offset bound being
    /// 200: `offset + duration <= 580 < 600`, so a flash always completes
    /// inside the interval that scheduled it. That is what makes
    /// [`EndFlashState::tick`]'s parameter-recalculation-before-`old_intensity`
    /// ordering unobservable — the parameters can never change mid-flash.
    #[test]
    fn flash_completes_within_its_interval() {
        // The structural bound, which needs no sampling: the offset is drawn
        // inclusively from `[0, 200]` and the duration from `[100, 380]`, so
        // the worst case is the sum of the two maxima.
        assert!(
            i64::from(MAX_FLASH_OFFSET_IN_TICKS + MAX_FLASH_DURATION_IN_TICKS)
                < FLASH_INTERVAL_IN_TICKS,
            "200 + 380 = 580 must be under the 600-tick interval"
        );

        // And the draws really do respect those bounds. Asserting the *observed*
        // maximum instead would pin a sample statistic — 4000 seeds only reach
        // 578, and widening the sample would silently change the expected number.
        for seed in 1..4000i64 {
            let (offset, duration, _, _) = oracle_params(seed, true);
            assert!((0..=MAX_FLASH_OFFSET_IN_TICKS).contains(&offset), "seed {seed}");
            assert!(
                (MIN_FLASH_DURATION_IN_TICKS..=MAX_FLASH_DURATION_IN_TICKS).contains(&duration),
                "seed {seed}"
            );
            assert!(i64::from(offset + duration) < FLASH_INTERVAL_IN_TICKS, "seed {seed}");
        }
    }

    // ---- the edge the sound hangs off --------------------------------------

    /// `flashStartedThisTick` fires one tick **after** `offset`, not on it.
    ///
    /// At `within == offset` the intensity is `Mth.sin(0)`, which is exactly
    /// `0.0`, and the predicate is `intensity > 0.0` — strictly. So the
    /// rising edge, and therefore the queued sound 30 ticks later, is at
    /// `offset + 1`. Reading "the flash starts at `offset`" off the schedule
    /// puts the sound one tick early.
    #[test]
    fn rising_edge_is_one_tick_after_the_offset() {
        let (offset, _, _, _) = oracle_params(1, true);
        let base = FLASH_INTERVAL_IN_TICKS; // interval seed 1

        let mut s = EndFlashState::default();
        let mut edges = Vec::new();
        for t in 0..base + FLASH_INTERVAL_IN_TICKS {
            s.tick(t);
            if s.flash_started_this_tick() {
                edges.push(t);
            }
        }
        // Stated twice on purpose: once derived, so the *rule* is legible,
        // and once as the absolute tick computed from the decompile's own
        // literals (600 + 133 + 1), so the witness does not move if the
        // interval constant does.
        assert_eq!(edges, vec![base + i64::from(offset) + 1]);
        assert_eq!(edges, vec![734]);

        // And the tick at `offset` itself is a genuine zero, not a rounding
        // artefact — that is the whole reason the edge moves.
        let at_offset = ticked_to(base + i64::from(offset));
        assert_eq!(at_offset.intensity(1.0), 0.0);
    }

    /// One interval, one flash — the counter the queued sound depends on.
    #[test]
    fn exactly_one_rising_edge_per_interval() {
        let mut s = EndFlashState::default();
        let mut per_interval = vec![0usize; 6];
        for t in 0..6 * FLASH_INTERVAL_IN_TICKS {
            s.tick(t);
            if s.flash_started_this_tick() {
                per_interval[(t / FLASH_INTERVAL_IN_TICKS) as usize] += 1;
            }
        }
        // Interval 0 never flashes (see `first_interval_never_flashes`);
        // every other interval flashes exactly once.
        assert_eq!(per_interval, vec![0, 1, 1, 1, 1, 1]);
    }

    // ---- the curve --------------------------------------------------------

    /// A half sine over `[offset, offset + duration]`: zero at the start,
    /// 1.0 at the midpoint, and back to (a hair above) zero at the end.
    ///
    /// The end is *not* exactly zero, and that is vanilla: M12 established
    /// that `Mth.sin(PI)` reads a tiny **positive** table entry where
    /// platform `sin(PI_f32)` is negative. A witness asserting `== 0.0`
    /// there would be asserting the platform's answer.
    ///
    /// Read off the tick field rather than through
    /// [`EndFlashState::intensity`] — see
    /// `lerp_to_one_is_not_a_select_at_the_flash_tail` for why those differ
    /// at exactly this point.
    #[test]
    fn intensity_is_a_half_sine() {
        let (offset, duration, _, _) = oracle_params(1, true);
        let base = FLASH_INTERVAL_IN_TICKS;
        let at = |within: i32| ticked_to(base + i64::from(within)).intensity;

        assert_eq!(at(offset), 0.0);
        let peak = at(offset + duration / 2);
        assert!((peak - 1.0).abs() < 1e-3, "midpoint intensity {peak}");
        let tail = at(offset + duration);
        assert!(tail > 0.0 && tail < 1e-3, "end-of-flash intensity {tail:e}");
        assert_eq!(at(offset + duration + 1), 0.0);
        assert_eq!(at(offset - 1), 0.0);
    }

    /// The intensity expression is `(t * PI) / duration`, left to right —
    /// Java's same-precedence operators associate leftward — and **not**
    /// `t * (PI / duration)`.
    ///
    /// The fixture is chosen, not convenient. Over every reachable
    /// `(duration, tick)` pair the two groupings reach a different
    /// `Mth.sin` table index in only **152 of 67721 cases (0.22%)**, because
    /// the table quantises away an ulp of difference almost everywhere — and
    /// they never differ at all for the duration `311` that seed 1 draws. So
    /// every other witness in this module is structurally blind to the
    /// grouping, and this one drives `calculate_intensity` directly with
    /// `duration = 112, t = 35`, which is one of the pairs that separates
    /// them.
    #[test]
    fn intensity_uses_javas_left_to_right_grouping() {
        let mut s = EndFlashState::default();
        // Any non-zero seed, so a `tick` would not redraw over these.
        s.flash_seed = 1;
        s.offset = 0;
        s.duration = 112;

        let ours = s.calculate_intensity(35);
        let regrouped = mth_sin(35.0 * (std::f32::consts::PI / 112.0));

        assert_ne!(
            ours.to_bits(),
            regrouped.to_bits(),
            "the fixture must be able to see the grouping at all"
        );
        assert_eq!(ours.to_bits(), 0x3F54_DB31, "left-to-right: 0.8314696");
        assert_eq!(regrouped.to_bits(), 0x3F54_D7B4, "regrouped: 0.83141637");
    }

    /// `Mth.lerp(1.0, a, b)` is `a + 1.0 * (b - a)`, **not** a select on `b`,
    /// and the difference is observable at the last tick of a flash: there
    /// `b` is `Mth.sin(PI)` ≈ `1.2e-16` while `a` is the previous tick's
    /// ≈ `0.0101`, so `b - a` rounds to exactly `-a` and the sum cancels to
    /// exactly `0.0`.
    ///
    /// So [`EndFlashState::flash_started_this_tick`], which reads the raw
    /// field, and every renderer, which reads the lerp, disagree about
    /// whether there is any flash at all on that tick. Vanilla has the same
    /// split, and at `1.2e-16` neither a pixel nor a listener can tell — the
    /// reason to pin it is that the same cancellation is what makes
    /// "`getIntensity(1.0)` is just this tick's value" a false shortcut for
    /// any future consumer.
    #[test]
    fn lerp_to_one_is_not_a_select_at_the_flash_tail() {
        let (offset, duration, _, _) = oracle_params(1, true);
        let s = ticked_to(FLASH_INTERVAL_IN_TICKS + i64::from(offset + duration));

        assert!(s.intensity > 0.0, "the tick value is a tiny positive sin(PI)");
        assert_eq!(s.intensity(1.0), 0.0, "the lerp to 1.0 cancels it away");
        assert!(
            s.old_intensity > 1e-3,
            "the cancellation needs a previous tick orders of magnitude larger, got {}",
            s.old_intensity
        );
    }

    /// `getIntensity(partialTicks)` is `Mth.lerp(partial, old, new)`
    /// (`Mth.java:550-552`) — `old + partial * (new - old)`, so `0.0` reads
    /// last tick and `1.0` reads this one. Swapping the endpoints would make
    /// the render lag a tick behind the state.
    #[test]
    fn intensity_lerps_from_old_to_new() {
        let (offset, duration, _, _) = oracle_params(1, true);
        let mut s = ticked_to(FLASH_INTERVAL_IN_TICKS + i64::from(offset) + i64::from(duration) / 2);
        let old = s.intensity(1.0);
        s.tick(FLASH_INTERVAL_IN_TICKS + i64::from(offset) + i64::from(duration) / 2 + 1);
        let new = s.intensity(1.0);

        assert_ne!(old.to_bits(), new.to_bits(), "the fixture must actually move");
        assert_eq!(s.intensity(0.0).to_bits(), old.to_bits());
        assert!((s.intensity(0.5) - (old + new) / 2.0).abs() < 1e-6);
    }

    // ---- ranges and edges --------------------------------------------------

    #[test]
    fn angles_stay_in_their_drawn_ranges() {
        for seed in 1..2000i64 {
            let (_, _, x, y) = oracle_params(seed, true);
            assert!((-60.0..10.0).contains(&x), "x_angle {x} for seed {seed}");
            assert!((-180.0..180.0).contains(&y), "y_angle {y} for seed {seed}");
        }
    }

    /// `clockTime / 600L` and `clockTime % 600L` are Java `long` operators,
    /// which truncate toward zero. Rust's `/` and `%` match; `div_euclid` /
    /// `rem_euclid` do not, and reaching for them is the plausible "fix".
    ///
    /// Observable because a truncating divide leaves `new_seed == 0` for a
    /// small negative clock, which equals the default `flash_seed`, so no
    /// draw happens and the parameters stay zeroed. Under `div_euclid` the
    /// seed would be `-1`, the draw would fire, and `offset` would move.
    #[test]
    fn negative_clock_truncates_toward_zero() {
        assert_eq!(-1i64 / FLASH_INTERVAL_IN_TICKS, 0);
        assert_eq!(-1i64 % FLASH_INTERVAL_IN_TICKS, -1);
        assert_eq!((-1i64).div_euclid(FLASH_INTERVAL_IN_TICKS), -1);

        let mut s = EndFlashState::default();
        s.tick(-1);
        assert_eq!(
            (s.offset, s.duration),
            (0, 0),
            "a truncating divide keeps seed 0, so no draw fires"
        );
        assert_eq!(s.intensity(1.0), 0.0);

        // The *remainder* needs its own fixture, and a fresh state cannot be
        // one: with `offset == duration == 0`, truncation puts `within` at
        // `-1` (below the window) and `rem_euclid` at `599` (above it), so
        // both readings fall outside and both answer `0.0`. A mutation to
        // `rem_euclid` survived until this half existed.
        //
        // Separating them needs a window wide enough for `600 + within` to
        // land *inside*: at `clock_time = -200` truncation gives `-200` and
        // `rem_euclid` gives `400`, which is well within `[100, 480]`.
        let mut wide = EndFlashState::default();
        wide.flash_seed = 1; // non-zero, so `calculate_intensity` sees these
        wide.offset = 100;
        wide.duration = 380;
        assert_eq!(
            wide.calculate_intensity(-200),
            0.0,
            "truncation puts within at -200, below the window"
        );
        // 400 is what `rem_euclid` would have produced, and it is 300/380 of
        // the way through the flash — `sin(0.789 π) ≈ 0.614`, so the two
        // readings differ by most of the curve's range rather than by a
        // rounding step.
        let euclid_reading = wide.calculate_intensity(400);
        assert!(
            euclid_reading > 0.6 && euclid_reading < 0.63,
            "expected mid-flash ≈0.614, got {euclid_reading}"
        );
    }
}
