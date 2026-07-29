//! The chunk-batch flow-control loop (M74): `chunk_batch_start`,
//! `chunk_batch_finished`, and the `chunk_batch_received` reply.
//!
//! **This module is a behaviour fix, not a missing decode.** Rewo already
//! answered `chunk_batch_finished` — with the literal `p.f32(64.0)`. Vanilla
//! answers `ChunkBatchSizeCalculator.getDesiredChunksPerTick()`, whose seeded
//! opening bid is **3.5**, so Rewo has been over-bidding the server by roughly
//! **18×** from the first batch of every session and never adapting. The
//! server sizes its chunk batches to that number, so this is a live
//! flow-control divergence rather than an unread float.
//!
//! `REWO_PACKET_COVERAGE.md` §4 listed `chunk_batch_finished` as a partial
//! whose unconsumed part was "the `batchSize` float". Two things about that
//! were wrong and are corrected here: the field is a **VarInt**, and the
//! *float* in this exchange is the serverbound reply's `desiredChunksPerTick`,
//! not anything the server sends.
//!
//! ## The loop
//!
//! Three packets, and Rewo was resolving one of them:
//!
//! | packet | body | effect |
//! |---|---|---|
//! | `chunk_batch_start` (12) | **empty** (`StreamCodec.unit`) | `onBatchStart()` — stamps the clock |
//! | `chunk_batch_finished` (11) | one **VarInt** `batchSize` | `onBatchFinished(batchSize)`, then reply |
//! | `chunk_batch_received` (sb) | one **f32** `desiredChunksPerTick` | the client's bid |
//!
//! Without `chunk_batch_start` there is no interval to measure, which is why
//! the two clientbound halves are one milestone: implementing the calculator
//! against a clock nothing ever stamps would produce a *different* constant,
//! not an adaptive one.
//!
//! ## The clock is a parameter
//!
//! Vanilla calls `Util.getNanos()` (i.e. `System.nanoTime()`) inside both
//! mutators. Here the caller passes it, so the arithmetic is deterministic and
//! a witness can drive a whole batch history without sleeping. The live
//! session passes a real monotonic clock; the tests below pass a script.
//!
//! ## Four rules where the plausible implementation is silently wrong
//!
//! Each is mutation-tested by a witness below.
//!
//! 1. **`batchSize > 0` guards the whole update.** A zero batch contributes no
//!    sample *and does not bump the weight*. Dropping the guard divides by
//!    zero — in Java `double / 0` is `Infinity`, not an exception — which
//!    poisons the running mean permanently and makes the bid `0.0`, i.e. the
//!    client asks the server to stop sending chunks. Nothing errors.
//! 2. **The clamp bounds are relative to the current aggregate**, recomputed
//!    on every sample: `agg / 3 ..= agg * 3`. They are not fixed nanosecond
//!    constants. This is what bounds how fast the estimate can move, and a
//!    fixed clamp lets one slow batch dominate.
//! 3. **The weight is used before it is bumped.** The mean is
//!    `(agg * w + clamped) / (w + 1)` and only *then* does `w` become
//!    `min(49, w + 1)`. Bumping first weights the first sample as if a sample
//!    had already been seen, and the error persists for the whole session
//!    because the mean is running.
//! 4. **The bid is `7e6 / agg`, computed in `double` and cast to `f32`.**
//!    With the seeded `agg` of `2e6` that is exactly **3.5**. Rewo's old
//!    constant was `64.0`.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/client/multiplayer/ChunkBatchSizeCalculator.java`
//! - `net/minecraft/network/protocol/game/ClientboundChunkBatchStartPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundChunkBatchFinishedPacket.java`
//! - `net/minecraft/network/protocol/game/ServerboundChunkBatchReceivedPacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleChunkBatchStart`, `handleChunkBatchFinished`
//! - `net/minecraft/util/Mth.java` — `clamp(double, double, double)`

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `ChunkBatchSizeCalculator.MAX_OLD_SAMPLES_WEIGHT`.
pub const MAX_OLD_SAMPLES_WEIGHT: i32 = 49;

/// `ChunkBatchSizeCalculator.CLAMP_COEFFICIENT`. Used as both the divisor and
/// the multiplier of the per-sample clamp window.
pub const CLAMP_COEFFICIENT: f64 = 3.0;

/// The field initialiser `aggregatedNanosPerChunk = 2000000.0` — 2 ms per
/// chunk, the estimate in force before any batch has been timed.
pub const SEED_NANOS_PER_CHUNK: f64 = 2_000_000.0;

/// The numerator of `getDesiredChunksPerTick`. 7 ms of a 50 ms tick.
pub const NANOS_BUDGET_PER_TICK: f64 = 7_000_000.0;

/// `ChunkBatchSizeCalculator` — the client's estimate of how long a chunk
/// costs it, and therefore how many it can absorb per tick.
///
/// Cloned freely; it is five machine words with no allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBatchSizeCalculator {
    /// `aggregatedNanosPerChunk` — a running weighted mean.
    aggregated_nanos_per_chunk: f64,
    /// `oldSamplesWeight`, starting at **1** and capped at 49.
    old_samples_weight: i32,
    /// `chunkBatchStartTime`. Vanilla seeds it with `Util.getNanos()` at
    /// construction; [`Self::new`] takes that same stamp so a
    /// `chunk_batch_finished` arriving before any `chunk_batch_start` measures
    /// from session start rather than from the epoch.
    chunk_batch_start_time: i64,
}

impl ChunkBatchSizeCalculator {
    /// `new ChunkBatchSizeCalculator()`, with the constructor's
    /// `Util.getNanos()` supplied by the caller.
    pub fn new(now_nanos: i64) -> Self {
        Self {
            aggregated_nanos_per_chunk: SEED_NANOS_PER_CHUNK,
            old_samples_weight: 1,
            chunk_batch_start_time: now_nanos,
        }
    }

    /// `onBatchStart` — stamp the clock. This is the entire body of
    /// `handleChunkBatchStart`, and the packet has an empty body, so there is
    /// nothing to decode.
    pub fn on_batch_start(&mut self, now_nanos: i64) {
        self.chunk_batch_start_time = now_nanos;
    }

    /// `onBatchFinished` — fold one timed batch into the running mean.
    ///
    /// Rule 1: `batchSize > 0` guards everything, including the weight bump.
    /// Rule 2: the clamp window is `agg / 3 ..= agg * 3`, recomputed here.
    /// Rule 3: the *old* weight is used in the mean, then bumped.
    pub fn on_batch_finished(&mut self, batch_size: i32, now_nanos: i64) {
        if batch_size <= 0 {
            return;
        }
        // `double batchDuration = Util.getNanos() - this.chunkBatchStartTime;`
        // — a `long` subtraction widened to `double`. Wrapping matches Java's
        // `long` arithmetic; a monotonic clock never actually wraps.
        let batch_duration = now_nanos.wrapping_sub(self.chunk_batch_start_time) as f64;
        let nanos_per_chunk = batch_duration / f64::from(batch_size);
        let clamped = mth_clamp(
            nanos_per_chunk,
            self.aggregated_nanos_per_chunk / CLAMP_COEFFICIENT,
            self.aggregated_nanos_per_chunk * CLAMP_COEFFICIENT,
        );
        let w = f64::from(self.old_samples_weight);
        self.aggregated_nanos_per_chunk =
            (self.aggregated_nanos_per_chunk * w + clamped) / (w + 1.0);
        self.old_samples_weight = MAX_OLD_SAMPLES_WEIGHT.min(self.old_samples_weight + 1);
    }

    /// `getDesiredChunksPerTick` — `(float)(7000000.0 / agg)`. Rule 4.
    ///
    /// The divide is `double`; only the result narrows. Computing it in `f32`
    /// throughout is a different number in the last bits, and the value goes
    /// on the wire.
    pub fn desired_chunks_per_tick(&self) -> f32 {
        (NANOS_BUDGET_PER_TICK / self.aggregated_nanos_per_chunk) as f32
    }

    /// The running estimate, exposed for witnesses and diagnostics.
    pub fn aggregated_nanos_per_chunk(&self) -> f64 {
        self.aggregated_nanos_per_chunk
    }

    /// The current sample weight, exposed for witnesses and diagnostics.
    pub fn old_samples_weight(&self) -> i32 {
        self.old_samples_weight
    }
}

/// `Mth.clamp(double, double, double)` — `value < min ? min : Math.min(value, max)`.
///
/// Written out rather than reached for as `f64::clamp` or `value.min(max)`,
/// both of which differ from Java at the edges: `f64::clamp` panics when
/// `min > max`, and Rust's `f64::min` returns the *other* operand for `NaN`
/// where Java's `Math.min` propagates it. This form matches Java on both —
/// `NaN < min` is false and `NaN > max` is false, so `NaN` falls through
/// unchanged, exactly as `Math.min(NaN, max)` does.
///
/// It should be unreachable anyway: rule 1's `batch_size > 0` guard is what
/// keeps `nanos_per_chunk` finite in the first place.
fn mth_clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `ClientboundChunkBatchFinishedPacket` — one **VarInt** `batchSize`.
///
/// Not a float. `REWO_PACKET_COVERAGE.md` §4 said float, which is the
/// serverbound reply's field; reading four raw bytes here consumes a body that
/// is usually one byte long.
pub fn read_chunk_batch_finished(body: &[u8]) -> Result<i32> {
    let mut r = PacketReader::new(body);
    r.varint()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 4, and the headline of the milestone: the opening bid is 3.5.
    ///
    /// MUTATION: restoring Rewo's old `64.0` constant, or seeding
    /// `aggregated_nanos_per_chunk` with anything but `2e6`, moves this.
    #[test]
    fn the_opening_bid_is_three_and_a_half_not_sixty_four() {
        let c = ChunkBatchSizeCalculator::new(0);
        assert_eq!(c.desired_chunks_per_tick(), 3.5);
        // Stated as the ratio too, because the ratio is the finding: Rewo's
        // old reply over-bid the server by this much on every session.
        let over_bid = 64.0 / c.desired_chunks_per_tick();
        assert!(
            (over_bid - 18.285_715).abs() < 1e-4,
            "old constant over-bid by {over_bid}"
        );
    }

    /// Rule 1. A zero-size batch is ignored *entirely* — no sample, no weight
    /// bump — so the estimate is bit-identical afterwards.
    ///
    /// MUTATION: dropping the `batch_size <= 0` early return. The divide then
    /// yields `inf`, `mth_clamp` pins it to `agg * 3`, and the estimate rises
    /// forever. Nothing errors and nothing logs; the bid just decays to zero.
    #[test]
    fn a_zero_size_batch_changes_nothing_at_all() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        let before = c;
        c.on_batch_start(0);
        c.on_batch_finished(0, 5_000_000);
        assert_eq!(c, before, "a zero batch must not move the estimate");
        assert_eq!(c.old_samples_weight(), 1, "nor bump the weight");
    }

    /// Rule 1's other half — a *negative* batch size is equally inert. Vanilla
    /// writes `> 0`, not `!= 0`.
    ///
    /// MUTATION: writing the guard as `batch_size == 0`. A negative size then
    /// produces a negative `nanos_per_chunk`, which clamps to `agg / 3` and
    /// silently drives the estimate down — i.e. the client bids *higher* the
    /// more malformed the packet.
    #[test]
    fn a_negative_batch_size_is_inert_too() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        let before = c;
        c.on_batch_start(0);
        c.on_batch_finished(-4, 5_000_000);
        assert_eq!(c, before);
    }

    /// Rule 2. The clamp window is `agg/3 ..= agg*3`, so a batch that was
    /// enormously slower than the estimate contributes exactly `agg * 3` and
    /// no more.
    ///
    /// The sample sits **far past** the bound rather than near it, and the
    /// assertion is against the value the bound produces, so a wider or
    /// narrower window both move it. Vanilla: agg=2e6, w=1, so a clamped
    /// sample of 6e6 gives (2e6*1 + 6e6)/2 = 4e6.
    ///
    /// MUTATION: replacing `CLAMP_COEFFICIENT` with any other number, or
    /// making the window absolute rather than relative to `agg`.
    #[test]
    fn a_wildly_slow_batch_contributes_exactly_three_times_the_estimate() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        c.on_batch_start(0);
        // 1 chunk that took a full second — 500× the 2 ms estimate.
        c.on_batch_finished(1, 1_000_000_000);
        assert_eq!(c.aggregated_nanos_per_chunk(), 4_000_000.0);
    }

    /// Rule 2, the other bound. A batch far *faster* than the estimate
    /// contributes exactly `agg / 3`.
    ///
    /// MUTATION: a one-sided clamp — clamping only the upper bound is the
    /// natural mistake, since "slow batches are the danger" is the intuition.
    /// Vanilla: (2e6*1 + 2e6/3)/2 = 1333333.333…
    #[test]
    fn a_wildly_fast_batch_contributes_exactly_a_third_of_the_estimate() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        c.on_batch_start(0);
        // 100 chunks in a microsecond — 10 ns each, 200× faster than the seed.
        c.on_batch_finished(100, 1_000);
        let expect = (SEED_NANOS_PER_CHUNK + SEED_NANOS_PER_CHUNK / 3.0) / 2.0;
        assert!((c.aggregated_nanos_per_chunk() - expect).abs() < 1e-6);
    }

    /// A sample **inside** the window passes through unclamped — the partner
    /// to the two bound witnesses, without which a clamp that pinned
    /// *everything* to a bound would still pass them both.
    ///
    /// MUTATION: `mth_clamp` returning `min` (or `max`) unconditionally.
    #[test]
    fn a_sample_inside_the_window_is_not_clamped() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        c.on_batch_start(0);
        // 2 chunks in 6 ms → 3 ms each, comfortably within 0.667e6..6e6.
        c.on_batch_finished(2, 6_000_000);
        // (2e6 * 1 + 3e6) / 2
        assert_eq!(c.aggregated_nanos_per_chunk(), 2_500_000.0);
    }

    /// Rule 3. The weight used in the mean is the value *before* the bump.
    ///
    /// The two readings differ on the very first sample, which is exactly
    /// where a wrong one is least visible: with w=1 the correct mean is
    /// `(agg + s) / 2`, and bumping first gives `(2*agg + s) / 3`. This drives
    /// a sample right at the upper bound so the two answers are far apart.
    ///
    /// MUTATION: moving the `old_samples_weight` bump above the mean.
    #[test]
    fn the_weight_is_used_before_it_is_bumped() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        c.on_batch_start(0);
        c.on_batch_finished(1, 1_000_000_000); // clamps to 6e6
        assert_eq!(c.aggregated_nanos_per_chunk(), 4_000_000.0);
        // The bump-first reading would be (2e6*2 + 6e6)/3 = 3_333_333.33…
        assert_ne!(c.aggregated_nanos_per_chunk(), 10_000_000.0 / 3.0);
        assert_eq!(c.old_samples_weight(), 2);
    }

    /// Rule 3's cap. The weight saturates at 49 and stays there, so the mean's
    /// responsiveness has a floor.
    ///
    /// The loop runs well past the cap and the witness samples **on** the
    /// boundary — 48 → 49 → 49 — because a `<` / `<=` slip in the cap is
    /// invisible one step either side of it.
    ///
    /// MUTATION: `MAX_OLD_SAMPLES_WEIGHT` of 50, or an uncapped bump.
    #[test]
    fn the_sample_weight_saturates_at_forty_nine() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        for _ in 0..47 {
            c.on_batch_start(0);
            c.on_batch_finished(1, 2_000_000);
        }
        assert_eq!(c.old_samples_weight(), 48, "one below the cap");
        c.on_batch_start(0);
        c.on_batch_finished(1, 2_000_000);
        assert_eq!(c.old_samples_weight(), 49, "exactly the cap");
        c.on_batch_start(0);
        c.on_batch_finished(1, 2_000_000);
        assert_eq!(c.old_samples_weight(), 49, "and it stays there");
    }

    /// The clock is an interval, not a timestamp: only the difference between
    /// `on_batch_start` and `on_batch_finished` matters, so the same batch
    /// measured a day later gives the same estimate.
    ///
    /// MUTATION: using the finish stamp as the duration (i.e. forgetting to
    /// subtract `chunk_batch_start_time`). That is the shape of the bug
    /// `chunk_batch_start` exists to prevent, and with a real `nanoTime` the
    /// absolute value is huge, so the estimate would pin to the upper clamp
    /// on every single batch and the bid would collapse.
    #[test]
    fn only_the_interval_matters_not_the_absolute_clock() {
        let mut a = ChunkBatchSizeCalculator::new(0);
        a.on_batch_start(0);
        a.on_batch_finished(4, 8_000_000);

        let base = 86_400_000_000_000_i64; // a day in nanos
        let mut b = ChunkBatchSizeCalculator::new(base);
        b.on_batch_start(base);
        b.on_batch_finished(4, base + 8_000_000);

        assert_eq!(a.aggregated_nanos_per_chunk(), b.aggregated_nanos_per_chunk());
    }

    /// Without a `chunk_batch_start` the interval is measured from the
    /// *constructor's* stamp — vanilla seeds `chunkBatchStartTime` there, so a
    /// `chunk_batch_finished` that arrives first is still a finite measurement
    /// rather than one against the epoch.
    ///
    /// MUTATION: defaulting `chunk_batch_start_time` to `0` instead of taking
    /// the constructor's clock. With a real `nanoTime` (~hours of uptime, not
    /// zero) that measures the whole machine uptime as one batch, pinning the
    /// estimate to the upper clamp on the first packet of the session.
    #[test]
    fn the_constructor_seeds_the_clock_so_a_finish_without_a_start_is_finite() {
        let base = 500_000_000_000_i64;
        let mut c = ChunkBatchSizeCalculator::new(base);
        c.on_batch_finished(2, base + 4_000_000); // 2 ms each — inside the window
        assert_eq!(c.aggregated_nanos_per_chunk(), 2_000_000.0);
    }

    /// The estimate converges toward a steady batch rate rather than jumping
    /// to it, and the bid rises accordingly. This is the property that makes
    /// the whole loop adaptive, which the old constant was not.
    ///
    /// MUTATION: any change that stops the mean moving (e.g. never bumping
    /// the weight is caught elsewhere; here, a mean that ignores the new
    /// sample entirely) leaves the bid pinned at 3.5.
    #[test]
    fn a_fast_steady_client_bids_upward_over_successive_batches() {
        let mut c = ChunkBatchSizeCalculator::new(0);
        let first = c.desired_chunks_per_tick();
        let mut prev = c.aggregated_nanos_per_chunk();
        for _ in 0..20 {
            c.on_batch_start(0);
            // 10 chunks in 2 ms → 200 µs each, ten times faster than the seed.
            c.on_batch_finished(10, 2_000_000);
            assert!(
                c.aggregated_nanos_per_chunk() < prev,
                "the estimate must fall monotonically toward the true cost"
            );
            prev = c.aggregated_nanos_per_chunk();
        }
        assert!(
            c.desired_chunks_per_tick() > first,
            "a fast client must end up bidding above the opening 3.5"
        );
    }

    /// The body is a VarInt, not a float.
    ///
    /// MUTATION: reading `r.f32()`. A one-byte body then fails outright, and a
    /// five-byte one decodes to nonsense — but the arm swallows decode errors,
    /// so the visible symptom is only that the estimate stops adapting.
    #[test]
    fn chunk_batch_finished_reads_a_var_int() {
        // 0x80 0x01 == 128 as a VarInt, two bytes.
        assert_eq!(read_chunk_batch_finished(&[0x80, 0x01]).unwrap(), 128);
        // One byte is a complete body; a float reader would demand four.
        assert_eq!(read_chunk_batch_finished(&[0x19]).unwrap(), 25);
        assert!(read_chunk_batch_finished(&[]).is_err());
    }

    /// An overlong VarInt is rejected rather than read as something wider —
    /// M67's mutation survivor, which established that "the field is this many
    /// bytes in the happy case" is not the same claim as "the field is a
    /// VarInt".
    ///
    /// MUTATION: `r.varlong()? as i32`, which accepts the sixth continuation
    /// byte a VarInt reader must refuse.
    #[test]
    fn an_overlong_var_int_batch_size_is_rejected() {
        assert!(read_chunk_batch_finished(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).is_err());
    }
}
