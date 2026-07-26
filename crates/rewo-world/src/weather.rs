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
