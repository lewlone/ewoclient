//! Rain and thunder, and where each falls as snow (M33).
//!
//! Two independent pieces meet here.
//!
//! **The levels** are pure state off `ClientboundGameEventPacket`. The one
//! thing to know is that the client does **not** interpolate them: vanilla's
//! `Level.setRainLevel` writes the clamped value to *both* `oRainLevel` and
//! `rainLevel`, so `getRainLevel(partialTick)` is a `Mth.lerp` between two
//! identical numbers — a step. The smoothing you see in game is server-side;
//! `ServerLevel` broadcasts `RAIN_LEVEL_CHANGE` on every tick the value moves.
//! Adding a client-side fade would look right and be wrong.
//!
//! **The precipitation** is `Biome.getPrecipitationAt` — whether a column gets
//! rain, snow, or nothing. It needs real world-gen noise, because above
//! `seaLevel + 17` vanilla perturbs the temperature with a simplex, and the
//! `FROZEN` temperature modifier samples two more. Those reuse [`crate::
//! biome_noise`]'s `LegacyRandom` + `SimplexNoise`, already ported and
//! Temurin-verified for M14's swamp modifier.

use crate::biome_noise::{LegacyRandom, SimplexNoise};

/// `Biome.Precipitation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precipitation {
    None,
    Rain,
    Snow,
}

/// `Biome.TemperatureModifier`. M14's biome parse dropped this as
/// "color-irrelevant"; it decides snow-vs-rain, so M33 keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TemperatureModifier {
    #[default]
    None,
    Frozen,
}

impl TemperatureModifier {
    /// The registry string, as it appears in the biome registry NBT.
    pub fn from_name(name: &str) -> Self {
        match name {
            "frozen" => Self::Frozen,
            _ => Self::None,
        }
    }
}

/// The client's rain and thunder levels.
///
/// Both are plain values, not tracks — see the module docs on why there is no
/// `o`-prefixed previous value to interpolate against.
#[derive(Clone, Copy, Debug, Default)]
pub struct WeatherState {
    rain: f32,
    thunder: f32,
}

impl WeatherState {
    /// `ClientboundGameEventPacket` ids. Only these four are weather; the
    /// packet carries a dozen unrelated things on the same byte.
    pub const START_RAINING: u8 = 1;
    pub const STOP_RAINING: u8 = 2;
    pub const RAIN_LEVEL_CHANGE: u8 = 7;
    pub const THUNDER_LEVEL_CHANGE: u8 = 8;

    /// Apply one `game_event`. Returns whether it was a weather event at all,
    /// so a caller can tell "handled" from "some other game event".
    ///
    /// **`START_RAINING` sets the level to 0 and `STOP_RAINING` sets it to 1.**
    /// That is not a transcription slip — it is
    /// `ClientPacketListener.handleGameEvent` verbatim. The names describe the
    /// server's weather transition, and the client is setting the value the
    /// server's `RAIN_LEVEL_CHANGE` ramp will start *from*: rain begins at 0
    /// and climbs, and stops from 1 and falls. "Fixing" the apparent inversion
    /// would make rain snap to full the instant it begins.
    pub fn apply_game_event(&mut self, event: u8, param: f32) -> bool {
        match event {
            Self::START_RAINING => self.set_rain(0.0),
            Self::STOP_RAINING => self.set_rain(1.0),
            Self::RAIN_LEVEL_CHANGE => self.set_rain(param),
            Self::THUNDER_LEVEL_CHANGE => self.set_thunder(param),
            _ => return false,
        }
        true
    }

    /// `Level.setRainLevel` — clamped, and written to both slots.
    pub fn set_rain(&mut self, level: f32) {
        self.rain = level.clamp(0.0, 1.0);
    }

    /// `Level.setThunderLevel`.
    pub fn set_thunder(&mut self, level: f32) {
        self.thunder = level.clamp(0.0, 1.0);
    }

    /// `Level.getRainLevel(partialTick)`.
    pub fn rain_level(&self) -> f32 {
        self.rain
    }

    /// `Level.getThunderLevel(partialTick)` — **multiplied by the rain level**,
    /// so thunder only darkens weather that is already falling.
    pub fn thunder_level(&self) -> f32 {
        self.thunder * self.rain
    }

    /// Reset on a dimension change, the way a fresh `ClientLevel` starts.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// -- the precipitation rule ---------------------------------------------------

/// The climate inputs `Biome.getPrecipitationAt` reads.
#[derive(Clone, Copy, Debug)]
pub struct BiomeClimate {
    pub has_precipitation: bool,
    pub temperature: f32,
    pub temperature_modifier: TemperatureModifier,
}

impl Default for BiomeClimate {
    fn default() -> Self {
        Self {
            has_precipitation: true,
            temperature: 0.5,
            temperature_modifier: TemperatureModifier::None,
        }
    }
}

/// The three world-gen noises `Biome`'s climate code holds as statics.
///
/// Vanilla builds them once per JVM; this is the same, built once per client.
pub struct ClimateNoise {
    /// `TEMPERATURE_NOISE` — `LegacyRandomSource(1234)`, octaves `{0}`.
    temperature: SimplexNoise,
    /// `FROZEN_TEMPERATURE_NOISE` — `LegacyRandomSource(3456)`, octaves
    /// `{-2, -1, 0}`.
    frozen: PerlinSimplex3,
    /// `BIOME_INFO_NOISE` — `LegacyRandomSource(2345)`, octaves `{0}`.
    info: SimplexNoise,
}

impl Default for ClimateNoise {
    fn default() -> Self {
        Self::new()
    }
}

impl ClimateNoise {
    pub fn new() -> Self {
        Self {
            temperature: SimplexNoise::new(&mut LegacyRandom::new(1234)),
            frozen: PerlinSimplex3::new(3456),
            info: SimplexNoise::new(&mut LegacyRandom::new(2345)),
        }
    }

    /// `Biome.getPrecipitationAt(pos, seaLevel)`.
    pub fn precipitation_at(
        &self,
        climate: &BiomeClimate,
        x: i32,
        y: i32,
        z: i32,
        sea_level: i32,
    ) -> Precipitation {
        if !climate.has_precipitation {
            return Precipitation::None;
        }
        // `coldEnoughToSnow` is `!warmEnoughToRain`, and `warmEnoughToRain` is
        // `getTemperature(pos, seaLevel) >= 0.15`.
        if self.temperature_at(climate, x, y, z, sea_level) >= 0.15 {
            Precipitation::Rain
        } else {
            Precipitation::Snow
        }
    }

    /// `Biome.getHeightAdjustedTemperature` (vanilla's `getTemperature` is that
    /// behind a 1024-entry memo, which changes no value).
    pub fn temperature_at(
        &self,
        climate: &BiomeClimate,
        x: i32,
        y: i32,
        z: i32,
        sea_level: i32,
    ) -> f32 {
        let base = self.modify_temperature(climate, x, z);
        let snow_level = sea_level + 17;
        if y > snow_level {
            // Note the mixed precision, which is vanilla's: the noise is
            // sampled from `float` inputs, scaled in double, then the whole
            // adjustment is done in float.
            let v = (self.temperature.get_value_2d(x as f32 as f64 / 8.0, z as f32 as f64 / 8.0)
                * 8.0) as f32;
            base - (v + y as f32 - snow_level as f32) * 0.05 / 40.0
        } else {
            base
        }
    }

    /// `Biome.TemperatureModifier.modifyTemperature`.
    fn modify_temperature(&self, climate: &BiomeClimate, x: i32, z: i32) -> f32 {
        match climate.temperature_modifier {
            TemperatureModifier::None => climate.temperature,
            TemperatureModifier::Frozen => {
                let large = self.frozen.value(x as f64 * 0.05, z as f64 * 0.05) * 7.0;
                let edge = self.info.get_value_2d(x as f64 * 0.2, z as f64 * 0.2);
                if large + edge < 0.3 {
                    let small = self.info.get_value_2d(x as f64 * 0.09, z as f64 * 0.09);
                    if small < 0.8 {
                        return 0.2;
                    }
                }
                climate.temperature
            }
        }
    }
}

/// `PerlinSimplexNoise` for the octave set `{-2, -1, 0}`.
///
/// Worked from the decompiled constructor rather than generalised: that octave
/// set gives `lowFreqOctaves = 2`, `highFreqOctaves = 0`, three octaves, and a
/// `zeroOctaveIndex` of 0 — so all three levels are present (no `consumeCount`
/// gaps), and because `highFreqOctaves` is 0 the second `WorldgenRandom`
/// seeded from the zero octave's own value is never constructed. The factors
/// fall out as `highestFreqInputFactor = 2^0 = 1` and
/// `highestFreqValueFactor = 1 / (2^3 - 1) = 1/7`.
struct PerlinSimplex3 {
    levels: [SimplexNoise; 3],
}

impl PerlinSimplex3 {
    fn new(seed: i64) -> Self {
        let mut rng = LegacyRandom::new(seed);
        // Constructed in order: the zero octave first, then indices 1 and 2.
        // Each consumes the same 262 values from the shared stream, so the
        // order is load-bearing.
        let a = SimplexNoise::new(&mut rng);
        let b = SimplexNoise::new(&mut rng);
        let c = SimplexNoise::new(&mut rng);
        Self { levels: [a, b, c] }
    }

    /// `getValue(x, y, false)`.
    fn value(&self, x: f64, z: f64) -> f64 {
        let mut value = 0.0;
        let mut factor = 1.0;
        let mut value_factor = 1.0 / 7.0;
        for level in &self.levels {
            value += level.get_value_2d(x * factor, z * factor) * value_factor;
            factor /= 2.0;
            value_factor *= 2.0;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_raining_sets_zero_and_stop_raining_sets_one() {
        // The inversion is vanilla's. If this test ever "fails" because someone
        // made it intuitive, the render is what broke.
        let mut w = WeatherState::default();
        w.apply_game_event(WeatherState::START_RAINING, 0.0);
        assert_eq!(w.rain_level(), 0.0);
        w.apply_game_event(WeatherState::STOP_RAINING, 0.0);
        assert_eq!(w.rain_level(), 1.0);
    }

    #[test]
    fn the_level_is_a_step_not_a_ramp() {
        let mut w = WeatherState::default();
        w.apply_game_event(WeatherState::RAIN_LEVEL_CHANGE, 0.37);
        // There is no partial-tick argument to pass, by construction: vanilla
        // writes both slots, so nothing could interpolate.
        assert_eq!(w.rain_level(), 0.37);
    }

    #[test]
    fn thunder_is_gated_on_rain() {
        let mut w = WeatherState::default();
        w.apply_game_event(WeatherState::THUNDER_LEVEL_CHANGE, 1.0);
        assert_eq!(w.thunder_level(), 0.0, "thunder without rain is nothing");
        w.apply_game_event(WeatherState::RAIN_LEVEL_CHANGE, 0.5);
        assert_eq!(w.thunder_level(), 0.5);
    }

    #[test]
    fn levels_clamp_and_unrelated_events_are_ignored() {
        let mut w = WeatherState::default();
        w.apply_game_event(WeatherState::RAIN_LEVEL_CHANGE, 4.0);
        assert_eq!(w.rain_level(), 1.0);
        w.apply_game_event(WeatherState::RAIN_LEVEL_CHANGE, -2.0);
        assert_eq!(w.rain_level(), 0.0);
        // `CHANGE_GAME_MODE` is 3 — it must not touch the weather.
        w.set_rain(0.6);
        assert!(!w.apply_game_event(3, 1.0));
        assert_eq!(w.rain_level(), 0.6);
    }

    #[test]
    fn a_biome_without_precipitation_gets_none_however_cold() {
        let n = ClimateNoise::new();
        let desert = BiomeClimate {
            has_precipitation: false,
            temperature: 0.0,
            temperature_modifier: TemperatureModifier::None,
        };
        assert_eq!(
            n.precipitation_at(&desert, 0, 64, 0, 63),
            Precipitation::None
        );
    }

    #[test]
    fn the_snow_threshold_is_fifteen_hundredths() {
        let n = ClimateNoise::new();
        let warm = BiomeClimate {
            temperature: 0.15,
            ..Default::default()
        };
        let cold = BiomeClimate {
            temperature: 0.14,
            ..Default::default()
        };
        // At or below sea level + 17 the noise is not consulted at all, so
        // these are exact.
        assert_eq!(n.precipitation_at(&warm, 0, 64, 0, 63), Precipitation::Rain);
        assert_eq!(n.precipitation_at(&cold, 0, 64, 0, 63), Precipitation::Snow);
    }

    #[test]
    fn height_turns_rain_to_snow_above_sea_level_plus_seventeen() {
        let n = ClimateNoise::new();
        let plains = BiomeClimate {
            temperature: 0.8,
            ..Default::default()
        };
        // At y = 80 (= 63 + 17) the adjustment is off entirely.
        assert_eq!(
            n.temperature_at(&plains, 0, 80, 0, 63),
            0.8,
            "the rule is `y > seaLevel + 17`, strictly"
        );
        // High enough and 0.8 must fall under the threshold: the linear term
        // alone is (y - 80) * 0.00125, so it takes ~520 blocks plus whatever
        // the noise contributes. Mountains are colder, not warmer.
        let high = n.temperature_at(&plains, 0, 700, 0, 63);
        assert!(high < 0.8, "temperature must fall with height: {high}");
        assert_eq!(
            n.precipitation_at(&plains, 0, 700, 0, 63),
            Precipitation::Snow
        );
    }

    /// The frozen modifier must actually be reachable — a modifier that never
    /// fires would make `TemperatureModifier` dead weight and the biome-parse
    /// change pointless.
    #[test]
    fn the_frozen_modifier_pins_some_columns_to_two_tenths() {
        let n = ClimateNoise::new();
        let frozen = BiomeClimate {
            temperature: 0.8,
            temperature_modifier: TemperatureModifier::Frozen,
            ..Default::default()
        };
        let mut pinned = 0;
        for x in 0..64 {
            for z in 0..64 {
                if n.temperature_at(&frozen, x * 4, 64, z * 4, 63) == 0.2 {
                    pinned += 1;
                }
            }
        }
        assert!(
            pinned > 0 && pinned < 4096,
            "the ice-patch noise must vary, not pin everything or nothing: \
             {pinned} of 4096"
        );
    }

    /// The three-octave stack must not collapse to its first octave — that
    /// would be the easy way to get `PerlinSimplex3` subtly wrong.
    #[test]
    fn the_frozen_noise_uses_all_three_octaves() {
        let p = PerlinSimplex3::new(3456);
        let one_octave_only = |x: f64, z: f64| p.levels[0].get_value_2d(x, z) / 7.0;
        let mut differs = 0;
        // From 1, not 0: at the origin every octave samples (0, 0) and returns
        // zero, so a one-octave stack agrees with a three-octave one there for
        // reasons that have nothing to do with the octaves.
        for i in 1..=64 {
            let (x, z) = (i as f64 * 0.05, i as f64 * 0.11);
            if (p.value(x, z) - one_octave_only(x, z)).abs() > 1e-9 {
                differs += 1;
            }
        }
        assert_eq!(differs, 64);
    }
}

// -- the render-state extraction ---------------------------------------------

/// One weather column, as `WeatherEffectRenderer.ColumnInstance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnInstance {
    pub x: i32,
    pub z: i32,
    pub bottom_y: i32,
    pub top_y: i32,
    pub u_offset: f32,
    pub v_offset: f32,
    /// Packed the way `LightCoordsUtil.pack` does: `block << 4 | sky << 20`.
    pub light_coords: i32,
}

/// `LightCoordsUtil.pack`.
pub fn pack_light(block: i32, sky: i32) -> i32 {
    block << 4 | sky << 20
}

/// `LightCoordsUtil.block` / `.sky`.
pub fn light_block(packed: i32) -> i32 {
    (packed >> 4) & 15
}
pub fn light_sky(packed: i32) -> i32 {
    (packed >> 20) & 15
}

/// `WeatherEffectRenderer.createRainColumnInstance`.
///
/// The per-column seed is set by the caller (both column kinds share one
/// `RandomSource` reseeded per column, exactly as vanilla does).
pub fn rain_column(
    rng: &mut crate::biome_noise::LegacyRandom,
    game_time: i64,
    x: i32,
    bottom_y: i32,
    top_y: i32,
    z: i32,
    light_coords: i32,
    partial_ticks: f32,
) -> ColumnInstance {
    let wrapped_ticks = (game_time & 131_071) as i32;
    let tick_offset = (x.wrapping_mul(x).wrapping_mul(3121)
        + x.wrapping_mul(45_238_971)
        + z.wrapping_mul(z).wrapping_mul(418_711)
        + z.wrapping_mul(13_761))
        & 0xFF;
    let speed = 3.0 + rng.next_float();
    let texture_offset =
        -((wrapped_ticks + tick_offset) as f32 + partial_ticks) / 32.0 * speed;
    ColumnInstance {
        x,
        z,
        bottom_y,
        top_y,
        u_offset: 0.0,
        v_offset: texture_offset % 32.0,
        light_coords,
    }
}

/// `WeatherEffectRenderer.createSnowColumnInstance`.
///
/// Snow drifts rather than falling straight: its u and v both wander off a
/// gaussian, and its light is **brightened** — `(level * 3 + 15) / 4` on each
/// channel — so flakes stay visible against a dark sky.
pub fn snow_column(
    rng: &mut crate::biome_noise::LegacyRandom,
    game_time: i64,
    x: i32,
    bottom_y: i32,
    top_y: i32,
    z: i32,
    light_coords: i32,
    partial_ticks: f32,
) -> ColumnInstance {
    let wrapped_ticks = (game_time & 131_071) as i32;
    let time = wrapped_ticks as f32 + partial_ticks;
    let u = (rng.next_double() + (time * 0.01) as f64 * rng.next_gaussian()) as f32;
    let v = (rng.next_double() + time as f64 * rng.next_gaussian() * 0.001) as f32;
    let v_offset = -(((game_time & 511) as f32) + partial_ticks) / 512.0;
    let brightened = pack_light(
        (light_block(light_coords) * 3 + 15) / 4,
        (light_sky(light_coords) * 3 + 15) / 4,
    );
    ColumnInstance {
        x,
        z,
        bottom_y,
        top_y,
        u_offset: u,
        v_offset: v_offset + v,
        light_coords: brightened,
    }
}

/// The per-column seed: `x*x*3121 + x*45238971 ^ z*z*418711 + z*13761`.
///
/// Note the precedence — `^` binds looser than `+` in Java, so this is
/// `(x*x*3121 + x*45238971) ^ (z*z*418711 + z*13761)`, not a sum of four
/// terms. Reading it as left-to-right arithmetic gives a different world.
pub fn column_seed(x: i32, z: i32) -> i64 {
    let a = x
        .wrapping_mul(x)
        .wrapping_mul(3121)
        .wrapping_add(x.wrapping_mul(45_238_971));
    let b = z
        .wrapping_mul(z)
        .wrapping_mul(418_711)
        .wrapping_add(z.wrapping_mul(13_761));
    (a ^ b) as i64
}

#[cfg(test)]
mod column_tests {
    use super::*;
    use crate::biome_noise::LegacyRandom;

    /// `x*x*3121 + x*45238971 ^ z*z*418711 + z*13761` — `^` binds LOOSER than
    /// `+` in Java, so this is `(a) ^ (b)`, not a left-to-right chain. Reading
    /// it wrong reseeds every column and changes the whole rainfall.
    #[test]
    fn the_column_seed_xors_two_sums_rather_than_chaining() {
        let (x, z) = (37i32, -19i32);
        let a = x * x * 3121 + x * 45_238_971;
        let b = z * z * 418_711 + z * 13_761;
        assert_eq!(column_seed(x, z), (a ^ b) as i64);
        // The left-to-right misreading, for contrast.
        let chained = ((x * x * 3121 + x * 45_238_971) ^ (z * z * 418_711)) + z * 13_761;
        assert_ne!(column_seed(x, z), chained as i64);
    }

    /// Rain scrolls its texture down; snow drifts sideways as well. The
    /// distinguishing feature is that rain leaves `u` alone and snow does not.
    #[test]
    fn rain_falls_straight_and_snow_drifts() {
        let mut rng = LegacyRandom::new(column_seed(5, 9));
        let rain = rain_column(&mut rng, 1000, 5, 60, 90, 9, pack_light(0, 15), 0.0);
        assert_eq!(rain.u_offset, 0.0, "rain does not drift sideways");
        assert!(rain.v_offset <= 0.0 && rain.v_offset > -32.0);

        let mut rng = LegacyRandom::new(column_seed(5, 9));
        let snow = snow_column(&mut rng, 1000, 5, 60, 90, 9, pack_light(0, 15), 0.0);
        assert_ne!(snow.u_offset, 0.0, "snow drifts");
    }

    /// Snow brightens its light so flakes stay visible: `(level*3 + 15) / 4`,
    /// which lifts 0 to 3 and leaves 15 at 15.
    #[test]
    fn snow_brightens_its_light_and_rain_does_not() {
        let dark = pack_light(0, 0);
        let mut rng = LegacyRandom::new(1);
        let rain = rain_column(&mut rng, 0, 0, 0, 1, 0, dark, 0.0);
        assert_eq!(rain.light_coords, dark, "rain takes the light as sampled");

        let mut rng = LegacyRandom::new(1);
        let snow = snow_column(&mut rng, 0, 0, 0, 1, 0, dark, 0.0);
        assert_eq!(light_block(snow.light_coords), 3);
        assert_eq!(light_sky(snow.light_coords), 3);

        let mut rng = LegacyRandom::new(1);
        let bright = snow_column(&mut rng, 0, 0, 0, 1, 0, pack_light(15, 15), 0.0);
        assert_eq!(light_block(bright.light_coords), 15, "full stays full");
        assert_eq!(light_sky(bright.light_coords), 15);
    }

    /// The pack/unpack pair must round-trip, since snow reads its own input
    /// back out to brighten it.
    #[test]
    fn light_coords_round_trip() {
        for block in 0..16 {
            for sky in 0..16 {
                let p = pack_light(block, sky);
                assert_eq!((light_block(p), light_sky(p)), (block, sky));
            }
        }
    }

    /// The gaussian pair-cache means two draws cost one rejection loop. If the
    /// cache were dropped, snow's u and v would both come from a first draw and
    /// the drift would be wrong.
    #[test]
    fn the_gaussian_caches_its_pair() {
        let mut a = LegacyRandom::new(42);
        let first = a.next_gaussian();
        let second = a.next_gaussian();
        assert_ne!(first, second);
        // A fresh RNG must reproduce both, in order.
        let mut b = LegacyRandom::new(42);
        assert_eq!(b.next_gaussian(), first);
        assert_eq!(b.next_gaussian(), second);
    }
}
