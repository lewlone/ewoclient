//! `ChunkedSampleByteBuf` — float samples to the 16-bit PCM OpenAL is fed.
//!
//! **The one part of the decode path with an exact vanilla answer**, which is
//! why it is transcribed to the bit rather than approximated. Everything else
//! about decoding an Ogg Vorbis stream is a tolerance question — Vorbis I does
//! not mandate identical float output between implementations, so Rewo's
//! decoder cannot be graded against jorbis bit-for-bit — but this step is plain
//! arithmetic on whatever floats arrive, and it either matches or it does not.
//!
//! ```java
//! public void accept(final float sample) {
//!    ...
//!    int intVal = Mth.clamp((int)(sample * 32767.5F - 0.5F), -32768, 32767);
//!    this.currentBuffer.putShort((short)intVal);
//!    this.byteCount += 2;
//! }
//! ```
//! (`ChunkedSampleByteBuf.java:22-31`.)
//!
//! **Reproducing it is the fidelity claim, not a bug being copied.** An f32
//! pipeline end to end is measurably more accurate; vanilla's is lossy in a
//! specific way, and matching that is what "sounds like Minecraft" means here.
//! Someone will eventually read this as a pointless precision loss in a float
//! mixer and delete it — hence the literal vectors in the tests, which fail
//! loudly rather than drifting.

/// `Mth.clamp((int)(sample * 32767.5F - 0.5F), -32768, 32767)`.
///
/// Three details, each of which a plausible implementation gets wrong and none
/// of which is audible as an obvious fault:
///
/// * **`32767.5`, not `32767`.** The half is what makes `1.0` land exactly on
///   `32767` once the bias is subtracted.
/// * **The `-0.5` bias is applied BEFORE the cast.** It shifts the truncation
///   boundary by half a step, so dropping it changes roughly every other
///   sample by one — a uniform +1 DC-ish offset rather than noise.
/// * **The cast TRUNCATES toward zero; it does not floor.** Java's `(int)` on a
///   negative float rounds toward zero, so `-0.5` becomes `0` and not `-1`.
///   This is observable at silence: a floor implementation encodes digital
///   silence as `-1` rather than `0`, which is a constant DC offset on every
///   silent sample of every sound.
///
/// The clamp is applied to the already-truncated integer, so it is a clamp on
/// the result and not on the input.
pub fn quantise(sample: f32) -> i16 {
    // f32 throughout: vanilla's arithmetic is `float`, and doing it in f64 would
    // put the product on a different side of the truncation boundary for inputs
    // near a step.
    let scaled = sample * 32767.5 - 0.5;
    // `as i32` in Rust truncates toward zero exactly as Java's `(int)` does, and
    // saturates rather than wrapping on overflow or NaN — which the clamp below
    // would mask anyway, but is worth knowing is not UB.
    (scaled as i32).clamp(-32768, 32767) as i16
}

/// The declared buffer size, `bufferSize + 1 & -2` — rounded **up** to even.
///
/// Recorded because of the asymmetry beside it: `ChunkedSampleByteBuf`'s
/// constructor stores this rounded value but allocates its **first** buffer from
/// the raw argument (`ChunkedSampleByteBuf.java:17-18`), so an odd size would
/// give a first chunk one byte smaller than every later one. Both of vanilla's
/// callers pass even sizes, so the path is unreachable there.
///
/// **Rewo rounds both, which is a deliberate divergence**: a one-byte-short
/// first buffer cannot hold a whole 16-bit sample, and reproducing a latent
/// inconsistency that vanilla never reaches would be copying a bug rather than a
/// behaviour. Stated here rather than left for someone to find as a mismatch.
pub fn buffer_size(requested: usize) -> usize {
    (requested + 1) & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The endpoints, and the two constants that produce them.
    #[test]
    fn full_scale_maps_to_the_full_range() {
        // 1.0 * 32767.5 - 0.5 = 32767.0 exactly.
        assert_eq!(quantise(1.0), 32767);
        // -1.0 * 32767.5 - 0.5 = -32768.0 exactly. The range is asymmetric, and
        // this is the sample that shows the multiplier is 32767.5 and not 32768.
        assert_eq!(quantise(-1.0), -32768);
        // A 32767 multiplier would land 1.0 on 32766 once the bias is taken off.
        assert_ne!(quantise(1.0), 32766);
    }

    /// **Silence is zero, and only truncation makes it so.**
    ///
    /// `(int)(0.0 * 32767.5 - 0.5)` is `(int)(-0.5)`, which Java truncates
    /// toward zero. A `floor` gives `-1`, and the difference is a constant DC
    /// offset on every silent sample of every sound in the game — inaudible on
    /// its own and exactly the sort of thing that never gets diagnosed.
    #[test]
    fn silence_truncates_to_zero_rather_than_flooring_to_minus_one() {
        assert_eq!(quantise(0.0), 0);
        assert_eq!((-0.5f32).floor() as i32, -1, "what a floor would have given");
    }

    /// Truncation toward zero is visible on any negative sample, not just at 0.
    #[test]
    fn negative_samples_truncate_toward_zero() {
        // -0.5 * 32767.5 - 0.5 = -16384.25 -> -16384 by truncation, -16385 by floor.
        assert_eq!(quantise(-0.5), -16384);
        let floored = (-0.5f32 * 32767.5 - 0.5).floor() as i32;
        assert_eq!(floored, -16385, "the reading this test exists to exclude");
    }

    /// The bias moves the boundary, so it is not a rounding nicety.
    #[test]
    fn the_bias_shifts_the_truncation_boundary() {
        // Pick a sample that lands just above an integer before the bias and
        // just below it after: 2 / 32767.5 scales to exactly 2.0.
        let s = 2.0f32 / 32767.5;
        assert_eq!(quantise(s), 1, "with the bias");
        assert_eq!((s * 32767.5) as i32, 2, "without it");
    }

    /// The clamp catches out-of-range input rather than wrapping it.
    ///
    /// A decoder is allowed to emit samples slightly outside [-1, 1] — Vorbis
    /// does not clamp — and wrapping instead of clamping turns a mild overshoot
    /// into full-scale noise of the opposite sign, which is the loudest possible
    /// failure.
    #[test]
    fn out_of_range_input_clamps_rather_than_wrapping() {
        assert_eq!(quantise(1.5), 32767);
        assert_eq!(quantise(-1.5), -32768);
        assert_eq!(quantise(1000.0), 32767);
        assert_eq!(quantise(f32::INFINITY), 32767);
        assert_eq!(quantise(f32::NEG_INFINITY), -32768);
    }

    /// Monotonic across the range: a louder sample never encodes quieter.
    #[test]
    fn the_mapping_is_monotonic() {
        let mut prev = i16::MIN;
        let mut s = -1.0f32;
        while s <= 1.0 {
            let v = quantise(s);
            assert!(v >= prev, "sample {s} gave {v} after {prev}");
            prev = v;
            s += 1.0 / 4096.0;
        }
    }

    #[test]
    fn buffer_size_rounds_up_to_even() {
        assert_eq!(buffer_size(8), 8);
        assert_eq!(buffer_size(9), 10);
        assert_eq!(buffer_size(0), 0);
        // Even sizes are what both of vanilla's callers pass, which is why its
        // own first-buffer asymmetry is unreachable there.
        assert_eq!(buffer_size(4096), 4096);
    }
}
