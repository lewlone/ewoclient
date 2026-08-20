//! Swamp grass-modifier noise — a verbatim port of vanilla's
//! `Biome.BIOME_INFO_NOISE`.
//!
//! `BiomeSpecialEffects.GrassColorModifier.SWAMP` samples
//! `Biome.BIOME_INFO_NOISE.getValue(x * 0.0225, z * 0.0225, false)` and picks
//! one of two grass colors on the −0.1 threshold. `BIOME_INFO_NOISE` is
//! `new PerlinSimplexNoise(new WorldgenRandom(new LegacyRandomSource(2345L)),
//! ImmutableList.of(0))` — a single-octave (octave 0) Perlin-Simplex, which for
//! octave set `{0}` reduces to exactly one `SimplexNoise` sampled at the raw
//! input with `highestFreqInputFactor = 1` and `highestFreqValueFactor = 1`
//! (decompiled `PerlinSimplexNoise`). So the whole thing is one 2-D
//! `SimplexNoise.getValue(x, y)`.
//!
//! To reproduce it bit-for-bit two Java primitives must be exact: `java.util.
//! Random` (here `LegacyRandomSource` — a 48-bit LCG, `BitRandomSource`
//! `nextInt`/`nextDouble`), and the `SimplexNoise` permutation shuffle +
//! gradient dot. `WorldgenRandom` just wraps + counts; its `next(bits)`
//! delegates straight to the inner `LegacyRandomSource`, so the observable
//! stream is that of a plain `LegacyRandomSource(2345)`.

/// Java `LegacyRandomSource` (a `java.util.Random`-compatible 48-bit LCG) plus
/// the `BitRandomSource` default `nextInt`/`nextDouble`. Only the calls the
/// simplex constructor needs are provided.
#[derive(Clone)]
pub struct LegacyRandom {
    seed: i64,
    /// `MarsagliaPolarGaussian`'s cached second value. Vanilla keeps this on a
    /// lazily-created helper that `setSeed` resets; a fresh `LegacyRandom` per
    /// reseed is the same thing.
    next_gaussian_value: f64,
    have_next_gaussian: bool,
}

impl LegacyRandom {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const INCREMENT: i64 = 0xB;
    const MASK: i64 = (1 << 48) - 1;
    /// `BitRandomSource.DOUBLE_MULTIPLIER = 1.110223E-16F` — a **float**. In
    /// `combined * 1.110223E-16F`, `combined` is a `long`, so Java binary
    /// numeric promotion converts BOTH operands to `float` and multiplies in
    /// float, returning a float widened to double. That is *lower* precision
    /// than `java.util.Random`'s `long * 0x1.0p-53` double — load-bearing to
    /// match MC exactly (verified vs a temurin-25 port: `Rng(0).nextDouble()`
    /// == 0.7309677600860596, not the JDK-double 0.730967787376657).
    const DOUBLE_MULTIPLIER: f32 = 1.110223E-16_f32;
    /// `BitRandomSource.FLOAT_MULTIPLIER`.
    const FLOAT_MULTIPLIER: f32 = 5.9604645E-8_f32;

    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
            next_gaussian_value: 0.0,
            have_next_gaussian: false,
        }
    }

    /// `LegacyRandomSource.next(bits)`: advance the LCG and take the top `bits`.
    fn next(&mut self, bits: u32) -> i32 {
        // Java `long` arithmetic wraps; mask to 48 bits after.
        self.seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT)
            & Self::MASK;
        // `newSeed >> (48 - bits)`: the masked seed is non-negative, and for
        // bits <= 31 the result fits a non-negative i32. Cast via i64 shift.
        (self.seed >> (48 - bits)) as i32
    }

    /// `BitRandomSource.nextInt(bound)`.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        if bound & (bound - 1) == 0 {
            // Power of two: (int)((long)bound * next(31) >> 31).
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            // Java's rejection guard, with i32 wrapping arithmetic.
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    /// `BitRandomSource.nextDouble()` — **and this transcription is WRONG.**
    ///
    /// The bytecode is `l2d; ldc2_w double 1.1102230246251565E-16d; dmul`, so
    /// the multiply happens in **double**; this does it in float. Pinned, with
    /// the `javap` output and the reason it is not fixed here, by
    /// `tests::the_two_next_doubles_disagree_and_this_module_has_the_wrong_one`
    /// (M162). `crate::particles::LegacyRandom::next_double` is the right one.
    ///
    /// The paragraph on `DOUBLE_MULTIPLIER` above argues for the float reading
    /// from the decompiler's `1.110223E-16F` literal. That argument is correct
    /// about Java and wrong about this program: the constant is inlined from a
    /// field whose declared type is `double`, and Vineflower re-rendered it
    /// with a float suffix.
    pub fn next_double(&mut self) -> f64 {
        let upper = self.next(26) as i64;
        let lower = self.next(27) as i64;
        let combined = (upper << 27) + lower;
        (combined as f32 * Self::DOUBLE_MULTIPLIER) as f64
    }

    /// `BitRandomSource.nextFloat()` — `next(24) * FLOAT_MULTIPLIER`, all in
    /// float. Used by M33's rain columns for their fall speed.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * Self::FLOAT_MULTIPLIER
    }

    /// `BitRandomSource.nextLong()` -- `((long)next(32) << 32) + next(32)`.
    ///
    /// **Both halves are SIGNED**, which is the whole subtlety: `next(32)`
    /// takes the top 32 bits of the 48-bit seed and narrows to `int`, so the
    /// low word is sign-extended by the `+` and a `u32`-flavoured reading
    /// (`(hi << 32) | lo`) disagrees on every draw whose low word has its top
    /// bit set -- about half of them.
    ///
    /// Added by M162 for the explosion sound's seed, which is
    /// `ClientLevel.playLocalSound`'s `this.random.nextLong()`. The seed is
    /// not decoration: `SoundEngine::resolve` feeds it to
    /// `get_sound_seeded`, so it picks WHICH of `entity.generic.explode`'s
    /// four variants you hear. A constant would play the same one every time.
    ///
    /// [`crate::particles::LegacyRandom::next_long`] is the same transcription
    /// against the same interface; a test in this module drives both over the
    /// same seeds, because two copies of one formula is two chances to drift.
    pub fn next_long(&mut self) -> i64 {
        let upper = self.next(32) as i64;
        let lower = self.next(32) as i64;
        (upper << 32).wrapping_add(lower)
    }

    /// `MarsagliaPolarGaussian.nextGaussian()`, the polar method vanilla's
    /// `RandomSource.nextGaussian` delegates to.
    ///
    /// It generates values in **pairs** and caches the second, so a caller that
    /// takes two gaussians consumes only one rejection loop — M33's snow
    /// columns take exactly two, so the cache is load-bearing rather than an
    /// optimisation. `setSeed` resets the cache in vanilla, which is why this
    /// lives on the RNG rather than beside it.
    pub fn next_gaussian(&mut self) -> f64 {
        if self.have_next_gaussian {
            self.have_next_gaussian = false;
            return self.next_gaussian_value;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let radius_squared = x * x + y * y;
            // Rejection is on `>= 1.0` OR exactly zero — both endpoints.
            if radius_squared >= 1.0 || radius_squared == 0.0 {
                continue;
            }
            let multiplier = (-2.0 * radius_squared.ln() / radius_squared).sqrt();
            self.next_gaussian_value = y * multiplier;
            self.have_next_gaussian = true;
            return x * multiplier;
        }
    }
}

/// Vanilla `SimplexNoise` — verbatim from the decompile. Only the 2-D
/// `getValue` is needed for the swamp modifier.
pub struct SimplexNoise {
    p: [i32; 512],
    #[allow(dead_code)]
    xo: f64,
    #[allow(dead_code)]
    yo: f64,
    #[allow(dead_code)]
    zo: f64,
}

const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

impl SimplexNoise {
    pub fn new(random: &mut LegacyRandom) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0i32; 512];
        for (i, slot) in p.iter_mut().take(256).enumerate() {
            *slot = i as i32;
        }
        for ix in 0..256usize {
            let offset = random.next_int(256 - ix as i32) as usize;
            p.swap(ix, offset + ix);
        }
        Self { p, xo, yo, zo }
    }

    fn p(&self, x: i32) -> i32 {
        self.p[(x & 0xFF) as usize]
    }

    fn get_corner_noise_3d(index: i32, x: f64, y: f64, z: f64, base: f64) -> f64 {
        let mut t0 = base - x * x - y * y - z * z;
        if t0 < 0.0 {
            return 0.0;
        }
        t0 *= t0;
        let g = GRADIENT[index as usize];
        t0 * t0 * (g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z)
    }

    /// `SimplexNoise.getValue(xin, yin)`.
    pub fn get_value_2d(&self, xin: f64, yin: f64) -> f64 {
        const SQRT_3: f64 = 1.7320508075688772; // Math.sqrt(3.0)
        let f2 = 0.5 * (SQRT_3 - 1.0);
        let g2 = (3.0 - SQRT_3) / 6.0;

        let s = (xin + yin) * f2;
        let i = (xin + s).floor() as i32;
        let j = (yin + s).floor() as i32;
        let t = (i + j) as f64 * g2;
        let x0_off = i as f64 - t;
        let y0_off = j as f64 - t;
        let x0 = xin - x0_off;
        let y0 = yin - y0_off;
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f64 + g2;
        let y1 = y0 - j1 as f64 + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;
        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = (self.p(ii + self.p(jj)) % 12) as i32;
        let gi1 = (self.p(ii + i1 + self.p(jj + j1)) % 12) as i32;
        let gi2 = (self.p(ii + 1 + self.p(jj + 1)) % 12) as i32;
        let n0 = Self::get_corner_noise_3d(gi0, x0, y0, 0.0, 0.5);
        let n1 = Self::get_corner_noise_3d(gi1, x1, y1, 0.0, 0.5);
        let n2 = Self::get_corner_noise_3d(gi2, x2, y2, 0.0, 0.5);
        70.0 * (n0 + n1 + n2)
    }
}

/// `Biome.BIOME_INFO_NOISE` — the swamp modifier's single-octave simplex,
/// seeded from `LegacyRandomSource(2345)`.
pub struct BiomeInfoNoise {
    noise: SimplexNoise,
}

impl Default for BiomeInfoNoise {
    fn default() -> Self {
        Self::new()
    }
}

impl BiomeInfoNoise {
    pub fn new() -> Self {
        let mut rng = LegacyRandom::new(2345);
        Self {
            noise: SimplexNoise::new(&mut rng),
        }
    }

    /// `BIOME_INFO_NOISE.getValue(x, z, false)` for the octave-`{0}` case:
    /// `highestFreqInputFactor = 1`, `highestFreqValueFactor = 1`, one octave,
    /// `useNoiseStart = false` → straight `simplex.getValue(x, z)`.
    pub fn value(&self, x: f64, z: f64) -> f64 {
        self.noise.get_value_2d(x, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ground truth from a faithful Java port of `LegacyRandomSource` +
    // `BitRandomSource` run under temurin-25 (see the M14 verification notes):
    //   Rng(2345).nextInt(100) == 40 ; Rng(0).nextInt(100) == 60
    //   Rng(0).nextDouble()    == 0.7309677600860596  <- WRONG, see M162: the
    //     JVM answers 0.730967787376657. The port this line cites reproduced
    //     the decompiler's float suffix rather than the bytecode's `dmul`.
    #[test]
    fn legacy_random_next_int_matches_java() {
        assert_eq!(LegacyRandom::new(2345).next_int(100), 40);
        assert_eq!(LegacyRandom::new(0).next_int(100), 60);
    }

    /// **Renamed by M162: this pins what this module DOES, not what the JVM
    /// does.** It used to be called `legacy_random_next_double_matches_java`
    /// and the JVM's answer is `0.730967787376657` — see
    /// `the_two_next_doubles_disagree_and_this_module_has_the_wrong_one` for
    /// the bytecode. Kept at the old value so the divergence stays visible
    /// rather than being quietly widened.
    #[test]
    fn legacy_random_next_double_is_the_float_multiply_this_module_ships() {
        let d = LegacyRandom::new(0).next_double();
        assert_eq!(d, 0.7309677600860596, "this module's own answer moved");
    }

    // Ground truth from the same Java port of `SimplexNoise` +
    // `BIOME_INFO_NOISE.getValue(x*0.0225, z*0.0225, false)`.
    #[test]
    fn swamp_noise_matches_java() {
        let n = BiomeInfoNoise::new();
        let cases: [(i32, i32, f64); 5] = [
            (0, 0, 0.0),
            (100, 100, 0.245_515_693_640_377_04),
            (-50, 200, -0.197_162_777_974_519_1),
            (7, -3, 0.539_014_071_814_936_8),
            (1000, 1000, -0.072_566_504_814_248_67),
        ];
        for (x, z, expect) in cases {
            let v = n.value(x as f64 * 0.0225, z as f64 * 0.0225);
            assert_eq!(v, expect, "swamp noise at ({x},{z}) != Java");
        }
    }

    // The −0.1 threshold branch: (-50,200) is below it (dark grass), (1000,
    // 1000) is above it (light grass) — the two swamp grass colors.
    #[test]
    fn swamp_threshold_splits_correctly() {
        let n = BiomeInfoNoise::new();
        assert!(n.value(-50.0 * 0.0225, 200.0 * 0.0225) < -0.1);
        assert!(n.value(1000.0 * 0.0225, 1000.0 * 0.0225) >= -0.1);
    }

    #[test]
    fn simplex_is_deterministic() {
        let a = BiomeInfoNoise::new();
        let b = BiomeInfoNoise::new();
        for i in 0..50 {
            let (x, z) = (i as f64 * 3.1, i as f64 * -1.7);
            assert_eq!(a.value(x, z), b.value(x, z));
        }
    }

    /// The two `LegacyRandom`s in this crate agree on three of four primitives
    /// — and finding the fourth is what this test is for (M162).
    ///
    /// `biome_noise` and `particles` each carry a transcription of the same
    /// `BitRandomSource`, written at different times and with different seed
    /// types (`i64` here, `u64` there). Nothing had ever compared them. Running
    /// this loop found that **`next_double` disagrees**; see
    /// `the_two_next_doubles_disagree_and_this_module_has_the_wrong_one`, which
    /// is why it is not in the loop below.
    ///
    /// **`next_long` is the one this milestone needed.** Its two halves are
    /// signed and it is the only primitive here whose plausible wrong version
    /// (a `|` over a zero-extended low word) disagrees on roughly half of all
    /// draws while looking identical.
    #[test]
    fn the_two_legacy_randoms_agree_draw_for_draw() {
        for seed in -50i64..50 {
            let mut a = LegacyRandom::new(seed);
            let mut b = crate::particles::LegacyRandom::new(seed);
            for _ in 0..4 {
                assert_eq!(a.next_long(), b.next_long(), "next_long @ {seed}");
                assert_eq!(a.next_float(), b.next_float(), "next_float @ {seed}");
                assert_eq!(a.next_int(7), b.next_int(7), "next_int @ {seed}");
            }
        }
    }

    /// A zero-extended low word is a DIFFERENT `next_long`, and the difference
    /// is not rare.
    ///
    /// The wrong reading agrees exactly when the low word is non-negative, so a
    /// witness that happened to pick such a seed would call the two equal.
    /// Measured over 200 seeds rather than asserted on one.
    #[test]
    fn sign_extension_of_the_low_word_is_load_bearing() {
        let mut disagreements = 0;
        for seed in 0i64..200 {
            let mut r = LegacyRandom::new(seed);
            let mut w = LegacyRandom::new(seed);
            let right = r.next_long();
            let (hi, lo) = (w.next(32), w.next(32));
            let wrong = ((hi as i64) << 32) | (lo as u32 as i64);
            if right != wrong {
                disagreements += 1;
            }
        }
        assert!(
            (60..=140).contains(&disagreements),
            "{disagreements} of 200 seeds disagree; expected roughly half"
        );
    }

    /// **A KNOWN BUG, pinned rather than fixed — this module's `next_double` is
    /// wrong and `crate::particles`' is right** (found by M162, whose own
    /// subject was `next_long`).
    ///
    /// Two transcriptions of one method disagree by ~2.7e-8 relative, and each
    /// has a doc comment claiming verification against a Temurin-25 Java port.
    /// Both ports cannot be right. **The bytecode settles it**, and settling it
    /// needed no reading of the decompile at all:
    ///
    /// ```text
    /// $ javap -c -p -constants -cp 26.2.jar \
    ///       net.minecraft.world.level.levelgen.BitRandomSource
    ///   public default double nextDouble();
    ///        26: lstore_3
    ///        27: lload_3
    ///        28: l2d                                    <-- long to DOUBLE
    ///        29: ldc2_w  double 1.1102230246251565E-16d <-- a DOUBLE constant
    ///        32: dmul                                   <-- DOUBLE multiply
    /// ```
    ///
    /// So `nextDouble` is `(double)combined * 2^-53`, which is what
    /// `particles::LegacyRandom::next_double` does and what its KAT (from a
    /// harness whose class bodies are copied verbatim from the decompile)
    /// records. This module does `(combined as f32 * MUL_f32) as f64`.
    ///
    /// **The trap, and it generalises past this function: a decompiler's
    /// numeric literal is not authoritative about its own TYPE.** Vineflower
    /// inlines `BitRandomSource`'s constant into `nextDouble`'s body and prints
    /// it `1.110223E-16F` — with a float suffix, which under JLS 5.6.2 would
    /// promote `long * float` to a *float* multiply and throw away 29 bits of
    /// the mantissa. That reading is self-consistent, is what this module's own
    /// doc argues for at length, and is wrong. The same decompiled file's
    /// **field declaration** (`double DOUBLE_MULTIPLIER = 1.110223E-16F;`)
    /// already says so, and the bytecode says so without needing care.
    ///
    /// **Not fixed here, deliberately.** `next_double` feeds `next_gaussian`,
    /// which feeds `SimplexNoise::new` (`weather.rs:157-159`) and therefore
    /// `BiomeInfoNoise` — so the fix moves M14's biome colours and M33's rain
    /// columns, whose pinned vectors came from the same Java port and would all
    /// need re-deriving against a JVM. That is a milestone, not a line, and
    /// bundling it into an unrelated sound change is how a wave of parallel
    /// branches becomes unmergeable. This test exists so the next reader finds
    /// the evidence rather than the argument.
    #[test]
    fn the_two_next_doubles_disagree_and_this_module_has_the_wrong_one() {
        let mine = LegacyRandom::new(0).next_double();
        let theirs = crate::particles::LegacyRandom::new(0).next_double();
        assert_ne!(mine, theirs, "if these ever agree, someone fixed it");
        // The `l2d; dmul` answer, which is `particles`' KAT vector 0 for seed 0
        // (`4604759192054975113` as bits) read back as a double.
        assert_eq!(theirs, f64::from_bits(4_604_759_192_054_975_113u64));
        assert_eq!(theirs, 0.730_967_787_376_657);
        // The `l2f; fmul; f2d` answer this module produces.
        assert_eq!(mine, 0.730_967_760_086_059_6);
        // And the constant itself is exactly 2^-53 under EITHER reading, which
        // is why the constant is not the bug and re-checking it does not help.
        assert_eq!(1.110_223e-16_f32 as f64, 2.0_f64.powi(-53));
    }
}
