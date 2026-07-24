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
pub struct LegacyRandom {
    seed: i64,
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

    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
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

    /// `BitRandomSource.nextDouble()` — the multiply happens in **float**
    /// (see `DOUBLE_MULTIPLIER`), then widens to double.
    pub fn next_double(&mut self) -> f64 {
        let upper = self.next(26) as i64;
        let lower = self.next(27) as i64;
        let combined = (upper << 27) + lower;
        (combined as f32 * Self::DOUBLE_MULTIPLIER) as f64
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
    //   Rng(0).nextDouble()    == 0.7309677600860596  (MC's float multiplier)
    #[test]
    fn legacy_random_next_int_matches_java() {
        assert_eq!(LegacyRandom::new(2345).next_int(100), 40);
        assert_eq!(LegacyRandom::new(0).next_int(100), 60);
    }

    #[test]
    fn legacy_random_next_double_matches_java() {
        let d = LegacyRandom::new(0).next_double();
        assert_eq!(d, 0.7309677600860596, "nextDouble(0) drifted from the JVM");
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
}
