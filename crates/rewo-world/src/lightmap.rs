//! rewo_world::lightmap — the CPU-side lightmap sampler, transcribed exactly
//! from the 26.2 client's GPU lightmap pipeline.
//!
//! Ground truth (all under the bundled 26.2 decompile,
//! `%APPDATA%/EwoClient/rewo/26.2/decompiled/`):
//!
//! - `assets/minecraft/shaders/core/lightmap.fsh` — the fragment shader that
//!   turns a `(block_level, sky_level)` texel into an RGB lightmap colour.
//!   [`sample`] is a line-for-line, IEEE-order port of its `main()`.
//! - `net/minecraft/client/renderer/LightmapRenderStateExtractor.java` —
//!   fills the shader's `LightmapInfo` uniforms each frame. The source for
//!   [`LightmapState`]'s fields, the block-light flicker
//!   ([`BlockLightFlicker`]), and the gamma/darkness terms
//!   ([`darkness_lightmap`]). `RandomSource.create()` there is a
//!   `LegacyRandomSource`, which is what [`LegacyRandom48`] ports.
//! - `net/minecraft/client/renderer/GameRenderer.java` `nightVisionScale` —
//!   [`night_vision_intensity`].
//! - `net/minecraft/client/Options.java` — `gamma` default `0.5`,
//!   `darknessEffectScale` default `1.0` (see [`darkness_lightmap`]).
//! - `net/minecraft/world/level/levelgen/LegacyRandomSource.java` +
//!   `BitRandomSource.java` — the exact 48-bit LCG ([`LegacyRandom48`]).
//! - `net/minecraft/util/Mth.java` — the 65 536-entry sine table
//!   ([`mth_sin`] / [`mth_cos`]).
//! - `net/minecraft/world/attribute/EnvironmentAttributes.java` — the default
//!   light colours, baked in as the constants below.
//!
//! Scope: this ports the clear-sky lightmap. Deliberately out of scope here
//! (later, per-context work): the end-flash sky boost and boss-overlay
//! world-darkening (`BossOverlayWorldDarkeningFactor` is pinned neutral `0.0`),
//! conduit-power water vision, and the *positional* (biome) interpolation the
//! attribute probe applies to the tint / night-vision colours.
//! `LightmapState` carries the already-resolved sky colour + factor **and the
//! resolved dimension ambient colour** (M16), so a caller can drive both
//! day/night and a dimension change; the remaining fixed colours are the
//! attribute defaults.

/// A compact, already-resolved lightmap uniform — the CPU mirror of the
/// shader's `LightmapInfo` block, minus the fields this scope pins to
/// constants (block tint = the default warm tint, night-vision colour =
/// `0x999999`, boss darkening = `0`).
///
/// `Default` is deliberately **visually neutral** so the old `demo`/`view`
/// replay paths — which predate any day/night driving — render exactly as
/// before: full sky factor, the vanilla `1.4` block factor, white sky light,
/// **the `EnvironmentAttributes` ambient default (black)**, and every
/// accessibility/effect term disabled. The live gamma of `0.5`
/// (`Options.gamma` default) is *not* baked into `Default`; rewo-app sets
/// `brightness_factor` from the player's actual gamma option later.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightmapState {
    /// `SkyFactor` — `EnvironmentAttributes.SKY_LIGHT_FACTOR`, default `1.0`.
    pub sky_factor: f32,
    /// `BlockFactor` — `blockLightFlicker + 1.4`. Default is the resting
    /// `1.4` (zero flicker).
    pub block_factor: f32,
    /// `SkyLightColor` — `EnvironmentAttributes.SKY_LIGHT_COLOR`, default
    /// white (`-1` → `1,1,1`); the day/night timeline tints it blue at night.
    pub sky_light_color: [f32; 3],
    /// `AmbientColor` — `EnvironmentAttributes.AMBIENT_LIGHT_COLOR` as
    /// `ARGB.vector3fFromRGB24` (a plain `/255`, **no** sRGB decode).
    ///
    /// This is the *dimension* attribute, not a constant: the attribute's
    /// codec default is `-16777216` (`0xFF000000` → black), and that is what
    /// [`LightmapState::default`] carries, but the built-in dimensions all
    /// override it — Overworld `#0a0a0a`, Nether `#302821`, End `#3f473f`
    /// (`data/minecraft/dimension_type/*.json`). No timeline track keyframes
    /// `visual/ambient_light_color` in 26.2 (`timeline/day.json` carries sky /
    /// fog / celestial tracks only), so within a dimension it is constant.
    pub ambient_color: [f32; 3],
    /// `BrightnessFactor` — the player's gamma option, minus the darkness
    /// effect (the `notGamma` mix weight). Default `0.0`.
    pub brightness_factor: f32,
    /// `DarknessScale` — the Darkness mob-effect subtraction. Default `0.0`.
    pub darkness_scale: f32,
    /// `NightVisionFactor` — Default `0.0` (no night vision).
    pub night_vision_factor: f32,
}

impl Default for LightmapState {
    fn default() -> Self {
        Self {
            sky_factor: 1.0,
            block_factor: 1.4,
            sky_light_color: [1.0, 1.0, 1.0],
            ambient_color: DEFAULT_AMBIENT_COLOR,
            brightness_factor: 0.0,
            darkness_scale: 0.0,
            night_vision_factor: 0.0,
        }
    }
}

// --- lightmap.fsh constants (attribute defaults, exact) ---

/// The `EnvironmentAttributes.AMBIENT_LIGHT_COLOR` **codec default**,
/// `-16777216` (`0xFF000000`) → RGB24 `0x000000`.
///
/// This is the value a dimension that sets no `minecraft:visual/
/// ambient_light_color` attribute resolves to — not "the Overworld's ambient".
/// The Overworld dimension type *does* set the attribute, to `#0a0a0a`; the
/// distinction is exactly why this is a `LightmapState` field and not a
/// constant folded into [`sample`]. Keeping the default at the codec default
/// leaves the serverless `demo` / `view` / `bench` paths — which have no
/// dimension registry at all — byte-identical to their pre-M16 output.
pub const DEFAULT_AMBIENT_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// `ARGB.vector3fFromRGB24(int)` — the low 24 bits as `r/255, g/255, b/255`.
/// A plain divide, **not** an sRGB decode: vanilla feeds these straight into
/// the lightmap shader's linear arithmetic, and the alpha byte is discarded.
pub fn rgb24_to_vec3(argb: i32) -> [f32; 3] {
    [
        ((argb >> 16) & 0xFF) as f32 / 255.0,
        ((argb >> 8) & 0xFF) as f32 / 255.0,
        (argb & 0xFF) as f32 / 255.0,
    ]
}

/// `NightVisionColor` — `EnvironmentAttributes.NIGHT_VISION_COLOR` default
/// `-6710887` (`0xFF999999`) → RGB24 `0x999999` → `153/255` per channel.
const NIGHT_VISION_COLOR: [f32; 3] = [153.0 / 255.0, 153.0 / 255.0, 153.0 / 255.0];

/// `BlockLightTint` — `EnvironmentAttributes.BLOCK_LIGHT_TINT` default
/// `-10100` (`0xFFFFD88C`) → RGB24 `0xFFD88C` → R 255, G 216, B 140.
const BLOCK_LIGHT_TINT: [f32; 3] = [255.0 / 255.0, 216.0 / 255.0, 140.0 / 255.0];

/// Boss-overlay world-darkening tint (`vec3(0.7, 0.6, 0.6)` in the shader).
const BOSS_DARKEN_TINT: [f32; 3] = [0.7, 0.6, 0.6];

/// `BossOverlayWorldDarkeningFactor`, pinned neutral for this scope.
const BOSS_FACTOR: f32 = 0.0;

/// `get_brightness(level)` from lightmap.fsh: `level / (4 - 3*level)`.
#[inline]
fn get_brightness(level: f32) -> f32 {
    level / (4.0 - 3.0 * level)
}

/// `parabolicMixFactor(level)` from lightmap.fsh: `(2*level - 1)^2`.
#[inline]
fn parabolic_mix_factor(level: f32) -> f32 {
    (2.0 * level - 1.0) * (2.0 * level - 1.0)
}

/// GLSL `mix(x, y, a)` for a scalar: `x*(1-a) + y*a`. Kept as the exact spec
/// form (not the `x + a*(y-x)` variant) so a NaN `y` with `a == 0` still
/// yields NaN, matching the shader's black-texel behaviour.
#[inline]
fn mix(x: f32, y: f32, a: f32) -> f32 {
    x * (1.0 - a) + y * a
}

/// Evaluate the lightmap fragment shader for one texel.
///
/// `block_level` / `sky_level` are the integer light levels `0..=15`; the
/// shader derives them from the texture coordinate as `floor(tc*16)/15`, so
/// the on-CPU equivalent is `level as f32 / 15.0`. **Invariant:** levels are
/// clamped to `15` here (the lightmap texture is 16×16; index 15 is the
/// brightest row/column). A caller should never pass a level above 15; if it
/// does, it is treated as 15 rather than producing an out-of-range texel.
///
/// Returns linear RGB in `[0, 1]` — except for the genuine `0/0` path: a
/// fully-dark texel (both levels 0, ambient black, no night vision)
/// makes `notGamma` compute `0.0/0.0 = NaN`. With `brightness_factor == 0`
/// the final `mix(color, NaN, 0.0)` is still `NaN` (IEEE `NaN * 0.0 = NaN`).
/// This is the exact source/CPU result of the shader math, faithfully
/// reproduced and not guarded. Rewo's permanent M13 `lightmapshot` oracle
/// renders this path through the production terrain shader and pins the
/// `R8G8B8A8_SRGB` readback to black `(0,0,0)`; the CPU API intentionally
/// retains the source NaN rather than baking that attachment conversion in.
pub fn sample(block_level: u8, sky_level: u8, state: &LightmapState) -> [f32; 3] {
    let block = block_level.min(15);
    let sky = sky_level.min(15);
    let block_level = block as f32 / 15.0;
    let sky_level = sky as f32 / 15.0;

    let block_brightness = get_brightness(block_level) * state.block_factor;
    let sky_brightness = get_brightness(sky_level) * state.sky_factor;

    // Ambient with or without night vision: max(ambient, nvColor * nvFactor).
    // `ambient` is the resolved dimension attribute, in the source's argument
    // order (`max(AmbientColor, NightVisionColor * NightVisionFactor)`).
    let mut color = [0.0f32; 3];
    for i in 0..3 {
        let night_vision = NIGHT_VISION_COLOR[i] * state.night_vision_factor;
        color[i] = state.ambient_color[i].max(night_vision);
    }

    // Add sky light.
    for i in 0..3 {
        color[i] += state.sky_light_color[i] * sky_brightness;
    }

    // Add block light: tint warmed toward white by 0.9 * parabolic(level).
    let mix_factor = 0.9 * parabolic_mix_factor(block_level);
    for i in 0..3 {
        let block_light_color = mix(BLOCK_LIGHT_TINT[i], 1.0, mix_factor);
        color[i] += block_light_color * block_brightness;
    }

    // Boss-overlay darkening (neutral 0 in this scope).
    for i in 0..3 {
        color[i] = mix(color[i], color[i] * BOSS_DARKEN_TINT[i], BOSS_FACTOR);
    }

    // Darkness-effect subtraction, then clamp.
    for i in 0..3 {
        color[i] -= state.darkness_scale;
    }
    for i in 0..3 {
        color[i] = color[i].clamp(0.0, 1.0);
    }

    // notGamma brightening, blended in by BrightnessFactor.
    let max_component = color[0].max(color[1]).max(color[2]);
    let max_inverted = 1.0 - max_component;
    let max_scaled = 1.0 - max_inverted * max_inverted * max_inverted * max_inverted;
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let not_gamma = color[i] * (max_scaled / max_component);
        out[i] = mix(color[i], not_gamma, state.brightness_factor);
    }
    out
}

// --- java.util.Random / LegacyRandomSource (exact LCG) ---

/// The vanilla 48-bit linear-congruential generator
/// (`LegacyRandomSource` / `java.util.Random`). `next_float` matches
/// `BitRandomSource.nextFloat` = `next(24) * 5.9604645e-8`.
///
/// This is the exact source `LightmapRenderStateExtractor` ticks for the
/// block-light flicker (`RandomSource.create()` builds a `LegacyRandomSource`).
/// The seed is an `i64` so the LCG's wrapping multiply reproduces Java's
/// signed-`long` overflow exactly; the `& MASK` keeps it in 48 positive bits.
#[derive(Clone, Debug)]
pub struct LegacyRandom48 {
    seed: i64,
}

impl LegacyRandom48 {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const INCREMENT: i64 = 11;
    const MASK: i64 = (1 << 48) - 1; // 281474976710655
    const FLOAT_MULTIPLIER: f32 = 5.9604645e-8; // 2^-24

    /// Deterministic seed, with the vanilla scramble
    /// `(seed ^ 0x5DEECE66D) & MASK`.
    pub fn with_seed(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    /// Non-deterministic seed (system time + a process-unique counter),
    /// analogous to `RandomSource.create()` / `generateUniqueSeed`.
    pub fn random() -> Self {
        use std::sync::atomic::{AtomicI64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicI64 = AtomicI64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let unique = COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E3779B97F4A7C15u64 as i64);
        Self::with_seed(nanos ^ unique)
    }

    /// `next(bits)` — advance the LCG and return the top `bits`.
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT)
            & Self::MASK;
        // seed is masked to 48 non-negative bits, so this arithmetic shift is
        // a logical shift; the result fits a positive i32 for bits <= 32.
        (self.seed >> (48 - bits)) as i32
    }

    /// `nextFloat()` in `[0, 1)`.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * Self::FLOAT_MULTIPLIER
    }
}

/// The block-light flicker term of `LightmapRenderStateExtractor.tick()`:
///
/// ```text
/// flicker += (nextFloat() - nextFloat()) * nextFloat() * nextFloat() * 0.1
/// flicker *= 0.9
/// ```
///
/// Four draws per tick, left-to-right exactly as Java evaluates the
/// expression. `block_factor()` is `flicker + 1.4` — precisely what the
/// extractor writes into `renderState.blockFactor`.
#[derive(Clone, Debug)]
pub struct BlockLightFlicker {
    rng: LegacyRandom48,
    flicker: f32,
}

impl BlockLightFlicker {
    /// Deterministic flicker (fixed seed) — for tests / replay parity.
    pub fn with_seed(seed: i64) -> Self {
        Self {
            rng: LegacyRandom48::with_seed(seed),
            flicker: 0.0,
        }
    }

    /// Non-deterministic flicker, matching the game's `RandomSource.create()`.
    pub fn random() -> Self {
        Self {
            rng: LegacyRandom48::random(),
            flicker: 0.0,
        }
    }

    /// Advance one client tick (four LCG draws), exactly as the extractor.
    pub fn tick(&mut self) {
        let a = self.rng.next_float();
        let b = self.rng.next_float();
        let c = self.rng.next_float();
        let d = self.rng.next_float();
        self.flicker = self.flicker + (a - b) * c * d * 0.1;
        self.flicker *= 0.9;
    }

    /// The raw flicker term (may be slightly negative).
    pub fn value(&self) -> f32 {
        self.flicker
    }

    /// `blockFactor` = flicker + 1.4.
    pub fn block_factor(&self) -> f32 {
        self.flicker + 1.4
    }
}

// --- Mth sine table (net/minecraft/util/Mth.java) ---

/// `Mth.SIN_SCALE` = `65536 / (2π)`, the sine-table index scale (verbatim
/// double literal from the decompile).
const MTH_SIN_SCALE: f64 = 10430.378350470453;

/// One `Mth` sine-table entry `SIN[rawIndex & 65535]`, evaluated on demand.
/// Vanilla precomputes `SIN[i] = (float)Math.sin(i / SIN_SCALE)`; `libm::sin`
/// is fdlibm-derived and matches Java's `Math.sin` at float precision (the
/// same equivalence the M12 celestial gate relies on), so this reproduces
/// each stored entry bit-for-bit without a 64 KiB table.
#[inline]
fn mth_sin_table(raw_index: i64) -> f32 {
    let idx = raw_index & 65535;
    libm::sin(idx as f64 / MTH_SIN_SCALE) as f32
}

/// `Mth.sin(float)` — `SIN[(int)((long)(v * SIN_SCALE) & 65535)]`. The `(long)`
/// cast truncates toward zero (Rust `as i64` on the widened float matches).
#[inline]
pub fn mth_sin(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE) as i64)
}

/// `Mth.cos(float)` — the same table offset a quarter turn (`+ 16384`).
#[inline]
pub fn mth_cos(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE + 16384.0) as i64)
}

// --- night vision + darkness lightmap terms ---

/// `GameRenderer.nightVisionScale`. `duration` is the effect's remaining
/// duration in ticks (`-1` = infinite); `partial` is the partial-tick.
///
/// `!nightVision.endsWithin(200)` → `1.0`, where `endsWithin(200)` is
/// `duration != -1 && duration <= 200`. So an infinite (`-1`) or
/// longer-than-200-tick effect returns `1.0`; within the last 200 ticks
/// (inclusive of 200) it pulses:
/// `0.7 + Mth.sin((duration - partial) * PI * 0.2) * 0.3`.
///
/// Note `duration - partial` is Java `int - float`: the int widens to `f32`
/// first (`duration as f32 - partial`).
pub fn night_vision_intensity(duration: i32, partial: f32) -> f32 {
    let ends_within = duration != -1 && duration <= 200;
    if !ends_within {
        return 1.0;
    }
    let angle = (duration as f32 - partial) * std::f32::consts::PI * 0.2;
    0.7 + mth_sin(angle) * 0.3
}

/// The gamma/darkness terms of `LightmapRenderStateExtractor.extract` +
/// `calculateDarknessScale`, returning `(brightness_factor, darkness_scale)`.
///
/// - `gamma` = `Options.gamma().get()` (default `0.5`).
/// - `darkness_option` = `Options.darknessEffectScale().get()` (default `1.0`).
/// - `blend` = `player.getEffectBlendFactor(DARKNESS, partial)` in `[0, 1]`.
/// - `tick_count` = `camera.tickCount` — a Java `int` (the player entity's
///   age). `tickCount - partial` is `int - float`, so the int widens to `f32`
///   (losing precision past 2²⁴); this takes `i32` and computes
///   `tick_count as f32 - partial` to match that semantics exactly.
///
/// ```text
/// modifier   = blend * darkness_option
/// brightness = max(0, gamma - modifier)
/// darkness   = max(0, Mth.cos((tickCount - partial) * PI * 0.025) * (0.45 * modifier)) * darkness_option
/// ```
pub fn darkness_lightmap(
    gamma: f32,
    darkness_option: f32,
    blend: f32,
    tick_count: i32,
    partial: f32,
) -> (f32, f32) {
    let modifier = blend * darkness_option;
    let brightness = (gamma - modifier).max(0.0);
    let cos = mth_cos((tick_count as f32 - partial) * std::f32::consts::PI * 0.025);
    let darkness = (cos * (0.45 * modifier)).max(0.0) * darkness_option;
    (brightness, darkness)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected bit pattern below was produced by an independent Java
    // oracle (a verbatim reimplementation of the decompiled classes) — NOT by
    // calling the functions under test — and the pins were independently
    // reproduced in JShell on Temurin JDK 25 (seed-0 `nextFloat` bits, all
    // eight flicker value/block-factor pairs, the `Mth.sin`/`cos` PI/0 bits,
    // and the four night-vision oracle bits). They pin the exact IEEE-754 f32
    // results the 26.2 client computes. The oracle source is scratch-only and
    // not committed.

    fn bits(f: f32) -> u32 {
        f.to_bits()
    }

    #[test]
    fn default_state_is_visually_neutral() {
        let s = LightmapState::default();
        assert_eq!(s.sky_factor, 1.0);
        assert_eq!(s.block_factor, 1.4);
        assert_eq!(s.sky_light_color, [1.0, 1.0, 1.0]);
        assert_eq!(s.brightness_factor, 0.0);
        assert_eq!(s.darkness_scale, 0.0);
        assert_eq!(s.night_vision_factor, 0.0);
        // The ambient default is the ATTRIBUTE codec default (0xFF000000 →
        // black), NOT the Overworld dimension's `#0a0a0a`. That distinction is
        // what keeps the serverless demo/view/bench paths byte-identical.
        assert_eq!(s.ambient_color, [0.0, 0.0, 0.0]);
        assert_eq!(s.ambient_color, DEFAULT_AMBIENT_COLOR);
        assert_eq!(
            rgb24_to_vec3(crate::dimension::DEFAULT_AMBIENT_LIGHT_COLOR),
            s.ambient_color
        );
        assert_ne!(
            rgb24_to_vec3(0x0A0A0A),
            s.ambient_color,
            "the Overworld dimension's #0a0a0a is a dimension attribute, not the default"
        );
    }

    /// `ARGB.vector3fFromRGB24` is a plain `/255` on the low 24 bits — no sRGB
    /// decode, alpha discarded. Pinned on the three built-in ambient colours.
    #[test]
    fn rgb24_to_vec3_is_a_plain_divide() {
        assert_eq!(rgb24_to_vec3(0xFF00_0000u32 as i32), [0.0, 0.0, 0.0]);
        // Overworld #0a0a0a = 10/255 per channel.
        let ow = rgb24_to_vec3(0xFF0A_0A0Au32 as i32);
        assert_eq!(bits(ow[0]), (10.0f32 / 255.0).to_bits());
        assert_eq!(ow, [10.0 / 255.0; 3]);
        // Nether #302821 = (48, 40, 33)/255 — the alpha byte must not leak in.
        assert_eq!(
            rgb24_to_vec3(0xFF30_2821u32 as i32),
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0]
        );
        // End #3f473f = (63, 71, 63)/255.
        assert_eq!(
            rgb24_to_vec3(0xFF3F_473Fu32 as i32),
            [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]
        );
        // sRGB decode would give ~0.0144, not 40/255 ≈ 0.1569 — reject it.
        assert!(rgb24_to_vec3(0xFF30_2821u32 as i32)[1] > 0.15);
    }

    /// The ambient term must OWN the fully-unlit pixel, not merely ride along
    /// in the struct: with block 0 / sky 0 and no night vision the whole
    /// lightmap colour IS the ambient colour (nothing else contributes), so a
    /// Nether ambient renders `#302821`'s exact float triple where the default
    /// renders the 0/0 NaN.
    #[test]
    fn ambient_owns_the_unlit_pixel() {
        const NETHER_AMBIENT: i32 = 0xFF30_2821u32 as i32;
        let s = LightmapState {
            ambient_color: rgb24_to_vec3(NETHER_AMBIENT),
            ..Default::default()
        };
        let out = sample(0, 0, &s);
        // brightness_factor is 0, so notGamma is mixed in at weight 0 and the
        // result is exactly the ambient — bit-for-bit.
        assert_eq!(bits(out[0]), (48.0f32 / 255.0).to_bits());
        assert_eq!(bits(out[1]), (40.0f32 / 255.0).to_bits());
        assert_eq!(bits(out[2]), (33.0f32 / 255.0).to_bits());
        // Same texel with the default (black) ambient is the documented NaN —
        // so this is a genuine behaviour change owned by the new field.
        assert!(sample(0, 0, &LightmapState::default())[0].is_nan());
    }

    /// Ambient is a floor under `max`, not an addend: once sky/block light
    /// exceeds it the pixel is unchanged, and night vision at full strength
    /// (0.6 grey) wins over the Nether's darker ambient channels.
    #[test]
    fn ambient_is_a_max_floor_in_source_order() {
        let nether = rgb24_to_vec3(0xFF30_2821u32 as i32);
        // Full sky: both states clamp to white — ambient cannot brighten past it.
        let a = sample(0, 15, &LightmapState { ambient_color: nether, ..Default::default() });
        let b = sample(0, 15, &LightmapState::default());
        assert_eq!(a, b, "a saturated texel is unaffected by ambient");

        // Night vision 1.0 seeds (0.6, 0.6, 0.6) — above every Nether channel,
        // so `max` picks night vision and the two agree exactly.
        let nv = LightmapState {
            ambient_color: nether,
            night_vision_factor: 1.0,
            ..Default::default()
        };
        let nv_default = LightmapState {
            night_vision_factor: 1.0,
            ..Default::default()
        };
        assert_eq!(sample(0, 0, &nv), sample(0, 0, &nv_default));

        // A *brighter* ambient than the night-vision seed wins instead, which
        // proves the `max` is evaluated per channel and not short-circuited.
        let bright = LightmapState {
            ambient_color: [0.9, 0.1, 0.1],
            night_vision_factor: 1.0,
            ..Default::default()
        };
        let out = sample(0, 0, &bright);
        assert_eq!(bits(out[0]), 0.9f32.to_bits(), "red takes the ambient");
        assert_eq!(
            bits(out[1]),
            (153.0f32 / 255.0).to_bits(),
            "green takes the night-vision seed"
        );
    }

    /// A mid-lit texel: ambient shifts a pixel that is neither black nor
    /// clamped, so the End's greenish `#3f473f` is visible on top of real sky
    /// and block light. Bit-pinned against the source expression evaluated in
    /// the same order.
    #[test]
    fn ambient_shifts_a_mid_lit_texel() {
        let end = rgb24_to_vec3(0xFF3F_473Fu32 as i32);
        let s = LightmapState {
            ambient_color: end,
            ..Default::default()
        };
        let plain = sample(4, 4, &LightmapState::default());
        let lit = sample(4, 4, &s);
        for c in 0..3 {
            assert!(
                lit[c] > plain[c] + 0.2,
                "channel {c}: ambient must lift the mid texel ({} vs {})",
                lit[c],
                plain[c]
            );
        }
        // The End ambient's green channel is the brightest, so the mid texel's
        // green lead over red must GROW versus the default (which is warm-red
        // dominated by the block tint).
        assert!(lit[1] - lit[0] > plain[1] - plain[0]);
    }

    #[test]
    fn legacy_random_seed0_first_floats() {
        // Java: new LegacyRandomSource(0), nextFloat() ×8.
        let expected = [
            0x3F3B20B4u32,
            0x3F54D951,
            0x3E764F2C,
            0x3F1B3970,
            0x3F232DC9,
            0x3E9E3BE0,
            0x3F0CE970,
            0x3DEFA128,
        ];
        let mut r = LegacyRandom48::with_seed(0);
        for (i, &want) in expected.iter().enumerate() {
            assert_eq!(bits(r.next_float()), want, "nextFloat #{i}");
        }
    }

    #[test]
    fn flicker_seed0_matches_java_oracle() {
        // (flicker value bits, block_factor bits) after each tick, seed 0.
        let expected = [
            (0xBAACDD13u32, 0x3FB307FCu32),
            (0x3A3BCC72, 0x3FB34AAD),
            (0xBA449C1E, 0x3FB31A9F),
            (0xBCA820AA, 0x3FB092B0),
            (0xBBBB10E1, 0x3FB27822),
            (0xBBADAC18, 0x3FB28587),
            (0xBB9C5BAC, 0x3FB296D7),
            (0x3CB3FE32, 0x3FB6032C),
        ];
        let mut fl = BlockLightFlicker::with_seed(0);
        for (t, &(want_f, want_bf)) in expected.iter().enumerate() {
            fl.tick();
            assert_eq!(bits(fl.value()), want_f, "flicker value at tick {}", t + 1);
            assert_eq!(
                bits(fl.block_factor()),
                want_bf,
                "block_factor at tick {}",
                t + 1
            );
        }
    }

    #[test]
    fn flicker_starts_at_rest() {
        let fl = BlockLightFlicker::with_seed(1);
        assert_eq!(fl.value(), 0.0);
        assert_eq!(fl.block_factor(), 1.4);
    }

    #[test]
    fn night_vision_intensity_boundaries() {
        // Infinite and > 200 ticks pin to exactly 1.0.
        assert_eq!(bits(night_vision_intensity(-1, 0.0)), 0x3F800000);
        assert_eq!(bits(night_vision_intensity(201, 0.0)), 0x3F800000);
        // At exactly 200, endsWithin(200) is true -> the pulse formula.
        assert_eq!(bits(night_vision_intensity(200, 0.5)), 0x3F1B7752);
        assert_eq!(bits(night_vision_intensity(200, 0.0)), 0x3F333333);
        // Near-expiry oscillation.
        assert_eq!(bits(night_vision_intensity(5, 0.0)), 0x3F333333);
        assert_eq!(bits(night_vision_intensity(100, 0.25)), 0x3F272E75);
    }

    #[test]
    fn darkness_lightmap_matches_java_oracle() {
        // (gamma, option, blend, tick, partial) -> (bright bits, scale bits).
        // blend 0 -> no darkness, brightness == gamma.
        let (b, d) = darkness_lightmap(0.5, 1.0, 0.0, 0, 0.0);
        assert_eq!((bits(b), bits(d)), (0x3F000000, 0x00000000));
        // t=22, modifier 1 -> cos negative -> darkness clamps to 0; gamma-1 -> 0.
        let (b, d) = darkness_lightmap(0.5, 1.0, 1.0, 22, 0.0);
        assert_eq!((bits(b), bits(d)), (0x00000000, 0x00000000));
        // gamma 0.5, option 0.25, blend 1, t=22, partial 0.5 -> brightness 0.25.
        let (b, d) = darkness_lightmap(0.5, 0.25, 1.0, 22, 0.5);
        assert_eq!((bits(b), bits(d)), (0x3E800000, 0x00000000));
        // t=0 cos=1: positive darkness. brightness 0.5-0.3 = 0.19999999 (IEEE).
        let (b, d) = darkness_lightmap(0.5, 1.0, 0.3, 0, 0.0);
        assert_eq!((bits(b), bits(d)), (0x3E4CCCCC, 0x3E0A3D71));
        // t=0, modifier 1 -> darkness = 0.45; brightness 0.5-1 -> 0.
        let (b, d) = darkness_lightmap(0.5, 1.0, 1.0, 0, 0.0);
        assert_eq!((bits(b), bits(d)), (0x00000000, 0x3EE66666));
    }

    #[test]
    fn mth_table_boundary_signs() {
        // Mth.sin(PI) reads a tiny POSITIVE table slot (sign flips vs true sin).
        assert_eq!(bits(mth_sin(std::f32::consts::PI)), 0x250D3132);
        assert!(mth_sin(std::f32::consts::PI) > 0.0);
        assert_eq!(bits(mth_cos(std::f32::consts::PI)), 0xBF800000); // -1.0
        assert_eq!(bits(mth_sin(0.0)), 0x00000000); // 0.0
        assert_eq!(bits(mth_cos(0.0)), 0x3F800000); // 1.0
    }

    #[test]
    fn sample_full_sky_is_white() {
        let s = LightmapState::default();
        let out = sample(0, 15, &s);
        assert_eq!(bits(out[0]), 0x3F800000);
        assert_eq!(bits(out[1]), 0x3F800000);
        assert_eq!(bits(out[2]), 0x3F800000);
    }

    #[test]
    fn sample_full_block_clamps_to_white() {
        let s = LightmapState::default();
        let out = sample(15, 0, &s);
        assert_eq!(bits(out[0]), 0x3F800000);
        assert_eq!(bits(out[1]), 0x3F800000);
        assert_eq!(bits(out[2]), 0x3F800000);
    }

    #[test]
    fn sample_mid_level_matches_java_oracle() {
        let s = LightmapState::default();
        let out = sample(7, 4, &s);
        assert_eq!(bits(out[0]), 0x3EAB52B6);
        assert_eq!(bits(out[1]), 0x3E97B996);
        assert_eq!(bits(out[2]), 0x3E63113C);
    }

    #[test]
    fn sample_mid_level_with_brightness_matches_java_oracle() {
        // brightness_factor 0.5 exercises the notGamma blend on a
        // non-degenerate (non-white, non-black) colour.
        let s = LightmapState {
            brightness_factor: 0.5,
            ..Default::default()
        };
        let out = sample(7, 4, &s);
        assert_eq!(bits(out[0]), 0x3F11BDA2);
        assert_eq!(bits(out[1]), 0x3F0111AD);
        assert_eq!(bits(out[2]), 0x3EC12941);
    }

    #[test]
    fn sample_black_texel_is_nan_via_0_over_0() {
        // block=0, sky=0, ambient black, no NV, brightness 0: notGamma computes
        // 0.0/0.0 = NaN; mix(color, NaN, 0.0) = NaN (IEEE NaN*0 = NaN). This is
        // the exact source/CPU result and is faithfully reproduced, not guarded.
        // The permanent M13 Vulkan oracle separately pins Rewo's production
        // R8G8B8A8_SRGB terrain readback for this source NaN to black.
        let s = LightmapState::default();
        let out = sample(0, 0, &s);
        assert!(out[0].is_nan(), "r should be NaN, got {}", out[0]);
        assert!(out[1].is_nan(), "g should be NaN, got {}", out[1]);
        assert!(out[2].is_nan(), "b should be NaN, got {}", out[2]);
    }

    #[test]
    fn sample_night_vision_lifts_black_texel_out_of_nan() {
        // Night vision seeds the ambient with (0.6,0.6,0.6), so max_component
        // is non-zero and the 0/0 singularity disappears.
        let s = LightmapState {
            night_vision_factor: 1.0,
            ..Default::default()
        };
        let out = sample(0, 0, &s);
        assert!(
            out.iter().all(|c| *c > 0.0 && !c.is_nan()),
            "expected a lit, finite texel, got {out:?}"
        );
    }

    #[test]
    fn sample_clamps_input_levels_to_15() {
        let s = LightmapState::default();
        // Levels above 15 are treated as 15 (documented invariant).
        assert_eq!(sample(200, 200, &s), sample(15, 15, &s));
    }
}
