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

// -- extraction from the world ------------------------------------------------

/// One extracted weather column, before it reaches the renderer.
///
/// The render-side twin is `rewo_gpu::weather::WeatherColumn`; the app converts.
/// The split follows vanilla's own: `WeatherRenderState` lives in the renderer
/// package, and only the extraction needs the level.
#[derive(Clone, Debug, Default)]
pub struct ExtractedWeather {
    pub intensity: f32,
    pub radius: i32,
    pub rain: Vec<ColumnInstance>,
    pub snow: Vec<ColumnInstance>,
}

impl ExtractedWeather {
    pub fn is_empty(&self) -> bool {
        self.rain.is_empty() && self.snow.is_empty()
    }
}

impl crate::World {
    /// `WeatherEffectRenderer.extractRenderState`.
    ///
    /// Returns an empty state the moment the intensity is zero — vanilla's
    /// `if (!(renderState.intensity <= 0.0F))` guard, which also means a NaN
    /// intensity produces nothing rather than a screen full of streaks.
    ///
    /// `radius` is vanilla's `weatherRadius` video option (default 10). It must
    /// stay ≤ 16 or the 32×32 direction table cannot address every column.
    pub fn extract_weather(
        &self,
        weather: &WeatherState,
        noise: &ClimateNoise,
        camera: [f64; 3],
        radius: i32,
        game_time: i64,
        partial_ticks: f32,
        sea_level: i32,
    ) -> ExtractedWeather {
        let intensity = weather.rain_level();
        let mut out = ExtractedWeather {
            intensity,
            radius,
            ..Default::default()
        };
        if !(intensity > 0.0) {
            return out;
        }
        let cam_x = camera[0].floor() as i32;
        let cam_y = camera[1].floor() as i32;
        let cam_z = camera[2].floor() as i32;
        for z in cam_z - radius..=cam_z + radius {
            for x in cam_x - radius..=cam_x + radius {
                let Some(terrain_height) = self.motion_blocking_height(x, z) else {
                    // No column loaded: vanilla would read a height of minY
                    // from an empty chunk and build a band anyway. Skipping is
                    // the honest choice — we have no terrain to hang rain on.
                    continue;
                };
                let y0 = (cam_y - radius).max(terrain_height);
                let y1 = (cam_y + radius).max(terrain_height);
                if y1 - y0 == 0 {
                    continue;
                }
                let Some(climate) = self.climate_at(x, cam_y, z) else {
                    continue;
                };
                let precipitation = noise.precipitation_at(&climate, x, cam_y, z, sea_level);
                if precipitation == Precipitation::None {
                    continue;
                }
                // One RNG reseeded per column, as vanilla does — and the seed
                // is a xor of two sums, not a chain (see `column_seed`).
                let mut rng = crate::biome_noise::LegacyRandom::new(column_seed(x, z));
                // The light is sampled at the higher of the camera and the
                // terrain, so rain in a valley is not lit by the cave you are
                // standing in.
                let light_sample_y = cam_y.max(terrain_height);
                let (block, sky) = self.light_at(x, light_sample_y, z);
                let coords = pack_light(block as i32, sky as i32);
                match precipitation {
                    Precipitation::Rain => out.rain.push(rain_column(
                        &mut rng,
                        game_time,
                        x,
                        y0,
                        y1,
                        z,
                        coords,
                        partial_ticks,
                    )),
                    Precipitation::Snow => out.snow.push(snow_column(
                        &mut rng,
                        game_time,
                        x,
                        y0,
                        y1,
                        z,
                        coords,
                        partial_ticks,
                    )),
                    Precipitation::None => unreachable!("filtered above"),
                }
            }
        }
        out
    }

    /// `level.getHeight(Heightmap.Types.MOTION_BLOCKING, x, z)`, or `None` when
    /// the column is not loaded or sent no such heightmap.
    pub fn motion_blocking_height(&self, x: i32, z: i32) -> Option<i32> {
        let col = self.column(x >> 4, z >> 4)?;
        let hm = col.motion_blocking.as_ref()?;
        Some(hm[(((z & 15) * 16) + (x & 15)) as usize])
    }

    /// The climate of the biome at a block position, or `None` without a biome
    /// context. Uses the same fiddled `BiomeManager.getBiome` lookup the tint
    /// stack does, so weather and grass colour agree about where they are.
    pub fn climate_at(&self, x: i32, y: i32, z: i32) -> Option<BiomeClimate> {
        let ctx = self.biome_context()?;
        let idx = ctx.get_biome(x, y, z, &|qx, qy, qz| self.noise_biome_at_quart(qx, qy, qz));
        let def = ctx.registry.get(idx as usize)?;
        Some(BiomeClimate {
            has_precipitation: def.has_precipitation,
            temperature: def.temperature,
            temperature_modifier: def.temperature_modifier,
        })
    }
}

#[cfg(test)]
mod extract_tests {
    use super::*;
    use crate::dimension::DimensionShape;

    /// Attach a one-biome registry so `climate_at` resolves. Temperature 0.8
    /// with precipitation is plains: rain, not snow, below the height cutoff.
    fn with_plains(w: &mut crate::World, climate: BiomeClimate) {
        use std::sync::Arc;
        let def = crate::biome::BiomeDef {
            name: "test:plains".into(),
            temperature: climate.temperature,
            downfall: 0.4,
            water_color: 0,
            grass_override: None,
            foliage_override: None,
            dry_foliage_override: None,
            grass_modifier: crate::biome::GrassModifier::None,
            sky_color: None,
            fog_color: None,
            has_precipitation: climate.has_precipitation,
            temperature_modifier: climate.temperature_modifier,
            ambient_sounds: None,
        };
        let registry = Arc::new(crate::biome::BiomeRegistry::new(vec![def]));
        w.set_biome_context(Arc::new(crate::biome::BiomeContext::new(
            registry,
            crate::biome::Colormaps::neutral(),
            0,
        )));
    }

    /// A world with one loaded column whose whole heightmap is `height`.
    fn world_at(height: i32) -> crate::World {
        let mut w = crate::World::new(DimensionShape::OVERWORLD);
        for cx in -1..=1 {
            for cz in -1..=1 {
                let mut col = crate::chunk::Column::empty_lit(&w.shape, cx, cz);
                col.motion_blocking = Some(Box::new([height; 256]));
                w.insert_column(cx, cz, col);
            }
        }
        w
    }

    /// Zero rain means no columns at all — the guard runs before any work.
    #[test]
    fn no_rain_extracts_nothing() {
        let w = world_at(64);
        let noise = ClimateNoise::new();
        let out = w.extract_weather(
            &WeatherState::default(),
            &noise,
            [0.0, 70.0, 0.0],
            4,
            0,
            0.0,
            63,
        );
        assert!(out.is_empty());
        assert_eq!(out.intensity, 0.0);
    }

    /// A column's band runs from the terrain height upward, never below it —
    /// this is the whole reason the heightmap had to be decoded.
    #[test]
    fn columns_start_at_the_terrain_not_below_it() {
        let mut w = world_at(80);
        with_plains(
            &mut w,
            BiomeClimate {
                temperature: 0.8,
                ..Default::default()
            },
        );
        let noise = ClimateNoise::new();
        let mut weather = WeatherState::default();
        weather.set_rain(1.0);
        // Camera just above the terrain with a radius that reaches below it:
        // `max(camY - radius, terrain)` must clamp the band's bottom up.
        let out = w.extract_weather(&weather, &noise, [0.5, 82.5, 0.5], 10, 0, 0.0, 63);
        assert!(!out.is_empty(), "a loaded, precipitating world must rain");
        for c in out.rain.iter().chain(out.snow.iter()) {
            assert!(c.bottom_y >= 80, "column {c:?} starts below the terrain");
            assert!(c.top_y > c.bottom_y);
        }
    }

    /// Deep underground there is no rain at all, and not because of a special
    /// case: `max(camY + r, terrain)` and `max(camY - r, terrain)` both
    /// collapse onto the terrain height, so the band has zero height and every
    /// column is skipped. Worth pinning, because it looks like a bug until you
    /// follow the two `max` calls.
    #[test]
    fn a_camera_far_below_the_terrain_sees_no_weather() {
        let mut w = world_at(80);
        with_plains(&mut w, BiomeClimate::default());
        let noise = ClimateNoise::new();
        let mut weather = WeatherState::default();
        weather.set_rain(1.0);
        let out = w.extract_weather(&weather, &noise, [0.5, 20.0, 0.5], 4, 0, 0.0, 63);
        assert!(out.is_empty());
    }

    /// An unloaded column contributes nothing rather than raining from minY.
    #[test]
    fn unloaded_columns_are_skipped() {
        let mut w = crate::World::new(DimensionShape::OVERWORLD);
        let mut col = crate::chunk::Column::empty_lit(&w.shape, 0, 0);
        col.motion_blocking = Some(Box::new([64; 256]));
        w.insert_column(0, 0, col);
        with_plains(&mut w, BiomeClimate::default());
        let noise = ClimateNoise::new();
        let mut weather = WeatherState::default();
        weather.set_rain(1.0);
        // Radius 10 from the origin reaches well past the single loaded chunk.
        let out = w.extract_weather(&weather, &noise, [8.5, 70.0, 8.5], 10, 0, 0.0, 63);
        for c in out.rain.iter().chain(out.snow.iter()) {
            assert!(
                (0..16).contains(&c.x) && (0..16).contains(&c.z),
                "column {c:?} is outside the one loaded chunk"
            );
        }
    }

    /// Without a biome context there is no climate, so nothing precipitates —
    /// a world that has not received its registry yet must not guess.
    #[test]
    fn no_biome_context_means_no_weather() {
        let w = world_at(64);
        assert!(w.biome_context().is_none(), "precondition");
        let noise = ClimateNoise::new();
        let mut weather = WeatherState::default();
        weather.set_rain(1.0);
        let out = w.extract_weather(&weather, &noise, [0.5, 70.0, 0.5], 4, 0, 0.0, 63);
        assert!(out.is_empty());
    }
}

// -- what rain does to the rest of the sky ------------------------------------

/// `AtmosphericFogEnvironment.applyWeatherDarken`.
///
/// Rain scales red and green by `1 - rain*0.5` but blue only by `1 - rain*0.4`;
/// thunder then scales all three equally. Alpha is untouched.
///
/// **This is a secondary touch-up, not where the sky greys.** It runs inside
/// `getBaseColor` on the SKY colour only (never on the fog colour), and on a
/// value that [`WeatherAttributes`] has already blended most of the way to grey.
/// Implementing this alone — which Rewo did at first — moves a rainy sky by
/// about 40% and leaves it obviously blue.
pub fn apply_weather_darken(color: i32, rain_level: f32, thunder_level: f32) -> i32 {
    let mut c = color;
    if rain_level > 0.0 {
        let m = 1.0 - rain_level * 0.5;
        let blue = 1.0 - rain_level * 0.4;
        c = scale_rgb(c, m, m, blue);
    }
    if thunder_level > 0.0 {
        let m = 1.0 - thunder_level * 0.5;
        c = scale_rgb(c, m, m, m);
    }
    c
}

/// `ARGB.scaleRGB(color, r, g, b)` — per channel, **truncating**, alpha kept.
fn scale_rgb(color: i32, r: f32, g: f32, b: f32) -> i32 {
    let u = color as u32;
    let ch = |shift: u32, s: f32| -> u32 {
        let v = ((u >> shift) & 0xFF) as f32;
        ((v * s) as i32).clamp(0, 255) as u32
    };
    ((u & 0xFF00_0000) | (ch(16, r) << 16) | (ch(8, g) << 8) | ch(0, b)) as i32
}

/// `SkyRenderState.rainBrightness = 1 - level.getRainLevel(partialTicks)`.
///
/// It becomes the sun's and the moon's **alpha**, so M12's celestials fade out
/// as rain comes in rather than shining through it.
pub fn rain_brightness(rain_level: f32) -> f32 {
    1.0 - rain_level
}

#[cfg(test)]
mod darken_tests {
    use super::*;

    /// Rain dims blue *less* than red and green, so the sky turns bluer as it
    /// darkens. A uniform scale would be the easy mistake.
    #[test]
    fn rain_dims_blue_less_than_red_and_green() {
        let white = 0xFFFF_FFFFu32 as i32;
        let out = apply_weather_darken(white, 1.0, 0.0) as u32;
        assert_eq!((out >> 16) & 0xFF, 127, "red halves");
        assert_eq!((out >> 8) & 0xFF, 127, "green halves");
        assert_eq!(out & 0xFF, 153, "blue keeps 60%");
        assert_eq!(out >> 24, 0xFF, "alpha untouched");
    }

    /// Thunder scales all three equally, on top of rain.
    #[test]
    fn thunder_dims_uniformly_and_compounds_with_rain() {
        let white = 0xFFFF_FFFFu32 as i32;
        let rain_only = apply_weather_darken(white, 1.0, 0.0) as u32;
        let both = apply_weather_darken(white, 1.0, 1.0) as u32;
        for shift in [16u32, 8, 0] {
            let a = (rain_only >> shift) & 0xFF;
            let b = (both >> shift) & 0xFF;
            assert_eq!(b, a / 2, "channel {shift} halves again");
        }
    }

    /// Clear weather changes nothing at all — the guards are `> 0.0`, so a
    /// zero level must not even truncate.
    #[test]
    fn clear_weather_is_the_identity() {
        for c in [0xFF78_A7FFu32 as i32, 0xFF00_0000u32 as i32, -1] {
            assert_eq!(apply_weather_darken(c, 0.0, 0.0), c);
        }
    }

    /// The celestials' alpha is the complement of the rain level.
    #[test]
    fn the_sun_fades_out_as_rain_comes_in() {
        assert_eq!(rain_brightness(0.0), 1.0);
        assert_eq!(rain_brightness(1.0), 0.0);
        assert_eq!(rain_brightness(0.25), 0.75);
    }
}

// -- `WeatherAttributes`: what rain actually does to the sky -------------------
//
// This, not `applyWeatherDarken`, is where a rainy sky goes grey. 26.2 moved
// weather's visual effect into the **environment attribute system**
// (`net/minecraft/world/attribute/WeatherAttributes.java`): RAIN and THUNDER are
// attribute *layers* that rewrite the resolved values before any renderer reads
// them. `AtmosphericFogEnvironment.applyWeatherDarken` still exists and still
// applies, but only to the sky colour inside fog-colour derivation, and on top
// of an already-greyed value.
//
// Two things about the layering are easy to get wrong. The two levels
// **partition** rather than stack: `rainLevel = getRainLevel() - thunderLevel`,
// so a full thunderstorm applies the THUNDER row alone. And the THUNDER
// modifier is applied to the RAIN row's *output*, not to the base.

/// `ARGB.greyscale` — the luma weights are 0.30 / 0.59 / 0.11, and the result
/// **truncates**.
pub fn greyscale(color: i32) -> i32 {
    let u = color as u32;
    let ch = |s: u32| ((u >> s) & 0xFF) as f32;
    let g = (ch(16) * 0.3 + ch(8) * 0.59 + ch(0) * 0.11) as u32;
    ((u & 0xFF00_0000) | (g << 16) | (g << 8) | g) as i32
}

/// `ARGB.multiply` — per channel, `a * b / 255`, **including alpha**. `-1`
/// (opaque white) on either side is the identity, which is the shortcut vanilla
/// takes and the reason a `#ffffffff` argument changes nothing.
pub fn multiply(lhs: i32, rhs: i32) -> i32 {
    if lhs == -1 {
        return rhs;
    }
    if rhs == -1 {
        return lhs;
    }
    let (l, r) = (lhs as u32, rhs as u32);
    let ch = |s: u32| (((l >> s) & 0xFF) * ((r >> s) & 0xFF) / 255) << s;
    (ch(24) | ch(16) | ch(8) | ch(0)) as i32
}

/// `ColorModifier.BLEND_TO_GRAY` — greyscale the subject, scale that to
/// `brightness`, then `srgbLerp` `factor` of the way toward it.
///
/// This is the whole reason a rainy sky greys out. At RAIN's `(0.6, 0.75)` the
/// sky ends up three quarters of the way to a 60%-bright grey.
pub fn blend_to_gray(color: i32, brightness: f32, factor: f32) -> i32 {
    let grey = scale_rgb(greyscale(color), brightness, brightness, brightness);
    crate::biome::srgb_lerp(factor, color, grey)
}

/// `ARGB.alphaBlend(destination, source)` — source-over, with vanilla's exact
/// integer rounding and its two shortcuts.
pub fn alpha_blend(destination: i32, source: i32) -> i32 {
    let (d, s) = (destination as u32, source as u32);
    let (da, sa) = ((d >> 24) & 0xFF, (s >> 24) & 0xFF);
    if sa == 255 {
        return source;
    }
    if sa == 0 {
        return destination;
    }
    let a = sa + da * (255 - sa) / 255;
    let ch = |shift: u32| -> u32 {
        let (dc, sc) = ((d >> shift) & 0xFF, (s >> shift) & 0xFF);
        if a == 0 {
            0
        } else {
            (sc * sa + dc * da * (255 - sa) / 255) / a
        }
    };
    ((a << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)) as i32
}

/// `Timelines.NIGHT_SKY_LIGHT_COLOR` — `colorFromFloat(1, 0.48, 0.48, 1)`.
pub const NIGHT_SKY_LIGHT_COLOR: i32 = 0xFF7A7AFFu32 as i32;

/// The visual attributes rain and thunder rewrite.
///
/// Every field is the *resolved* value the renderer would otherwise have used,
/// so [`WeatherAttributes::apply`] is a straight in-place transformation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherAttributes {
    pub sky_color: i32,
    pub fog_color: i32,
    pub cloud_color: i32,
    pub sky_light_level: f32,
    pub sky_light_color: i32,
    pub sky_light_factor: f32,
    pub star_brightness: f32,
    pub sunrise_sunset_color: i32,
}

impl WeatherAttributes {
    /// Apply the RAIN and then the THUNDER layer.
    ///
    /// `rain` and `thunder` are `Level.getRainLevel` and `getThunderLevel` —
    /// note the latter is already multiplied by the former, and that the rain
    /// row is driven by the **difference**.
    pub fn apply(&mut self, rain: f32, thunder: f32) {
        let rain_only = rain - thunder;
        if rain_only > 0.0 {
            self.layer(rain_only, Row::RAIN);
        }
        if thunder > 0.0 {
            self.layer(thunder, Row::THUNDER);
        }
    }

    fn layer(&mut self, level: f32, row: Row) {
        // `stateChangeLerp` is the type's own lerp: `srgbLerp` for colours and
        // `Mth.lerp` for floats (`AttributeType.ofInterpolated` passes one lerp
        // into all four slots).
        let lerp_c = |from: i32, to: i32| crate::biome::srgb_lerp(level, from, to);
        let lerp_f = |from: f32, to: f32| from + (to - from) * level;

        self.sky_color = lerp_c(
            self.sky_color,
            blend_to_gray(self.sky_color, row.sky_gray.0, row.sky_gray.1),
        );
        self.fog_color = lerp_c(self.fog_color, multiply(self.fog_color, row.fog_multiply));
        self.cloud_color = lerp_c(
            self.cloud_color,
            blend_to_gray(self.cloud_color, row.cloud_gray.0, row.cloud_gray.1),
        );
        // `FloatModifier.ALPHA_BLEND` is `Mth.lerp(alpha, subject, value)`.
        self.sky_light_level = lerp_f(
            self.sky_light_level,
            self.sky_light_level
                + (row.sky_light_level.0 - self.sky_light_level) * row.sky_light_level.1,
        );
        self.sky_light_color = lerp_c(
            self.sky_light_color,
            alpha_blend(
                self.sky_light_color,
                // `ARGB.color(alpha, NIGHT_SKY_LIGHT_COLOR)` replaces the alpha.
                (NIGHT_SKY_LIGHT_COLOR & 0x00FF_FFFF)
                    | (((row.sky_light_alpha * 255.0) as i32) << 24),
            ),
        );
        self.sky_light_factor = lerp_f(
            self.sky_light_factor,
            self.sky_light_factor
                + (row.sky_light_factor.0 - self.sky_light_factor) * row.sky_light_factor.1,
        );
        // `.set(...)`, not `.modify(...)`: stars are switched OFF, and the
        // layer lerps toward off. Rain does not dim them, it removes them.
        self.star_brightness = lerp_f(self.star_brightness, 0.0);
        self.sunrise_sunset_color = lerp_c(
            self.sunrise_sunset_color,
            multiply(self.sunrise_sunset_color, row.fog_multiply),
        );
    }
}

/// One row of `WeatherAttributes` — the RAIN or THUNDER modifier arguments,
/// transcribed literally.
#[derive(Clone, Copy)]
struct Row {
    sky_gray: (f32, f32),
    cloud_gray: (f32, f32),
    /// The `MULTIPLY_RGB` / `MULTIPLY_ARGB` argument, as ARGB.
    fog_multiply: i32,
    /// `FloatWithAlpha(value, alpha)`.
    sky_light_level: (f32, f32),
    sky_light_factor: (f32, f32),
    sky_light_alpha: f32,
}

impl Row {
    const RAIN: Self = Self {
        sky_gray: (0.6, 0.75),
        cloud_gray: (0.24, 0.5),
        // `colorFromFloat(1.0, 0.5, 0.5, 0.6)`.
        fog_multiply: 0xFF80_8099u32 as i32,
        sky_light_level: (4.0, 0.3125),
        sky_light_factor: (0.24, 0.3125),
        sky_light_alpha: 0.3125,
    };
    const THUNDER: Self = Self {
        sky_gray: (0.24, 0.94),
        cloud_gray: (0.095, 0.94),
        // `colorFromFloat(1.0, 0.25, 0.25, 0.3)`.
        fog_multiply: 0xFF40_404Cu32 as i32,
        sky_light_level: (4.0, 0.527_343_75),
        sky_light_factor: (0.24, 0.527_343_75),
        sky_light_alpha: 0.527_343_75,
    };
}

#[cfg(test)]
mod attribute_tests {
    use super::*;

    /// The Overworld's `visual/sky_color`.
    const OVERWORLD_SKY: i32 = 0xFF78A7FFu32 as i32;

    fn base() -> WeatherAttributes {
        WeatherAttributes {
            sky_color: OVERWORLD_SKY,
            fog_color: 0xFFC0D8FFu32 as i32,
            cloud_color: 0xCCFF_FFFFu32 as i32,
            sky_light_level: 15.0,
            sky_light_color: -1,
            sky_light_factor: 1.0,
            star_brightness: 1.0,
            sunrise_sunset_color: 0x80FF_A050u32 as i32,
        }
    }

    fn rgb(c: i32) -> (i32, i32, i32) {
        ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF)
    }

    /// The headline: a rainy sky is DESATURATED, not merely darker. The blue
    /// channel has to come down toward red and green, which
    /// `applyWeatherDarken` alone never does.
    #[test]
    fn full_rain_greys_the_sky_rather_than_only_dimming_it() {
        let mut a = base();
        a.apply(1.0, 0.0);
        let (r, _g, b) = rgb(a.sky_color);
        let (r0, _g0, b0) = rgb(OVERWORLD_SKY);
        let spread0 = b0 - r0;
        let spread = b - r;
        assert!(
            spread * 3 < spread0,
            "channel spread must collapse: {spread0} -> {spread}"
        );
        // #78a7ff (120, 167, 255) -> (106, 114, 136): nearly neutral, and
        // roughly half as bright. Both halves matter — a pure dim would keep
        // the spread, and a pure desaturate would keep the luma.
        assert!(
            (b as f32) < b0 as f32 * 0.6,
            "and it darkens substantially: {b0} -> {b}"
        );
        assert!(
            r < r0,
            "every channel comes down, not just the saturated one: {r0} -> {r}"
        );
    }

    /// Thunder is applied to the RAIN row's OUTPUT, and the two levels
    /// partition — a full thunderstorm is the THUNDER row alone.
    #[test]
    fn thunder_is_darker_than_rain_and_does_not_stack_with_it() {
        let (mut r, mut t) = (base(), base());
        r.apply(1.0, 0.0);
        t.apply(1.0, 1.0);
        assert!(rgb(t.sky_color).2 < rgb(r.sky_color).2, "thunder is darker");
        // rain - thunder == 0, so the rain row never runs.
        let mut only_thunder = base();
        only_thunder.layer(1.0, Row::THUNDER);
        assert_eq!(t.sky_color, only_thunder.sky_color);
    }

    #[test]
    fn clear_weather_is_the_identity() {
        let mut a = base();
        a.apply(0.0, 0.0);
        assert_eq!(a, base());
    }

    /// Stars are SET to zero, not scaled — at full rain there are none.
    #[test]
    fn rain_removes_the_stars() {
        let mut a = base();
        a.apply(1.0, 0.0);
        assert_eq!(a.star_brightness, 0.0);
        let mut half = base();
        half.apply(0.5, 0.0);
        assert_eq!(half.star_brightness, 0.5);
    }

    /// The lightmap darkens too — this is the piece a `client/`-only search for
    /// `getRainLevel` misses entirely, because it reaches the level through the
    /// attribute system.
    #[test]
    fn rain_darkens_the_sky_light() {
        let mut a = base();
        a.apply(1.0, 0.0);
        assert!(a.sky_light_level < 15.0, "{}", a.sky_light_level);
        assert!(a.sky_light_factor < 1.0, "{}", a.sky_light_factor);
        assert_ne!(a.sky_light_color, -1, "and it tints toward the night colour");
    }

    /// `ARGB.multiply` treats opaque white as the identity on either side.
    #[test]
    fn multiply_shortcuts_on_white() {
        assert_eq!(multiply(-1, 0x1234_5678), 0x1234_5678);
        assert_eq!(multiply(0x1234_5678, -1), 0x1234_5678);
    }

    /// `greyscale` uses luma weights, not a plain average — a saturated blue
    /// must come out dark, not mid-grey.
    #[test]
    fn greyscale_is_luma_weighted() {
        let blue = greyscale(0xFF0000FFu32 as i32) & 0xFF;
        assert_eq!(blue, 28, "0.11 * 255 truncated");
        let green = greyscale(0xFF00FF00u32 as i32) & 0xFF;
        assert_eq!(green, 150, "0.59 * 255 truncated");
    }
}

// -- the rain fog ramp --------------------------------------------------------

/// `AtmosphericFogEnvironment`'s `rainFogMultiplier` — the fourth and last
/// client consumer of the rain level, and the only one that moves a *distance*
/// rather than a colour.
///
/// It is **stateful**: the multiplier eases toward its target at
/// `deltaTicks * 0.2` per frame, so fog closes in over roughly five ticks
/// rather than snapping when the rain level changes. That easing is the whole
/// reason it lives on the fog environment in vanilla instead of being computed
/// fresh each frame.
///
/// Two inputs beyond the rain level, both easy to miss:
///
/// - **Sky light gates it.** `clamp((skyLight - 8) / 7, 0, 1)` means a camera
///   under 9 sky light gets *no* rain fog at all — which is why stepping into
///   a cave during a storm clears the air instantly.
/// - **A dry biome still gets half.** `rainsInBiome ? 1.0 : 0.5`: a desert
///   thickens too, even though no rain falls on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RainFog {
    multiplier: f32,
}

impl RainFog {
    /// `MIN_RAIN_FOG_SKY_LIGHT`.
    pub const MIN_SKY_LIGHT: f32 = 8.0;
    /// `RAIN_FOG_START_OFFSET`.
    pub const START_OFFSET: f32 = -160.0;
    /// `RAIN_FOG_END_OFFSET`.
    pub const END_OFFSET: f32 = -256.0;
    /// The floor the end distance may not fall below (`min(96, end)`).
    pub const MIN_END: f32 = 96.0;

    /// The value the multiplier is easing toward.
    pub fn target(rain_level: f32, sky_light: u8, rains_in_biome: bool) -> f32 {
        let sky = ((sky_light as f32 - Self::MIN_SKY_LIGHT) / 7.0).clamp(0.0, 1.0);
        rain_level * sky * if rains_in_biome { 1.0 } else { 0.5 }
    }

    /// `updateRainFogState` — one frame of easing.
    ///
    /// The lerp factor is clamped to 1, which vanilla does not do because its
    /// `deltaTicks` is always a fraction of a tick. A headless renderer that
    /// draws one frame after a long settle would otherwise overshoot past the
    /// target and oscillate.
    pub fn update(&mut self, rain_level: f32, sky_light: u8, rains_in_biome: bool, delta_ticks: f32) {
        let target = Self::target(rain_level, sky_light, rains_in_biome);
        let factor = (delta_ticks * 0.2).clamp(0.0, 1.0);
        self.multiplier += (target - self.multiplier) * factor;
    }

    /// Snap straight to the target, skipping the ease.
    ///
    /// For headless rendering only: a single frame drawn after the session has
    /// settled would otherwise show a multiplier still near zero, and grade a
    /// storm that has not arrived.
    pub fn converge(&mut self, rain_level: f32, sky_light: u8, rains_in_biome: bool) {
        self.multiplier = Self::target(rain_level, sky_light, rains_in_biome);
    }

    pub fn multiplier(&self) -> f32 {
        self.multiplier
    }

    /// Apply the offsets to a fog band.
    ///
    /// The offsets and the floor are vanilla's exactly. **The band they are
    /// applied to is not**: vanilla starts from `FOG_START_DISTANCE` /
    /// `FOG_END_DISTANCE` (0 and 1024 by default), and Rewo's world pass uses
    /// its own much tighter band so terrain dissolves into the sky at the
    /// render-distance edge rather than ending on a visible chunk boundary.
    /// So the *shape* of the change is faithful and its *scale* is relative to
    /// a different baseline; reading the real attributes is what would close
    /// that, and it would change clear-weather fog too.
    pub fn apply(&self, start: f32, end: f32) -> (f32, f32) {
        let new_start = start + Self::START_OFFSET * self.multiplier;
        let min_end = Self::MIN_END.min(end);
        let new_end = min_end.max(end + Self::END_OFFSET * self.multiplier);
        (new_start, new_end)
    }
}

#[cfg(test)]
mod rain_fog_tests {
    use super::*;

    /// Below 9 sky light there is no rain fog at all — the reason a cave is
    /// clear in a storm.
    #[test]
    fn sky_light_gates_it_off_underground() {
        assert_eq!(RainFog::target(1.0, 0, true), 0.0);
        assert_eq!(RainFog::target(1.0, 8, true), 0.0, "8 is the floor, exclusive");
        assert!(RainFog::target(1.0, 9, true) > 0.0);
        assert_eq!(RainFog::target(1.0, 15, true), 1.0, "full sky light, full fog");
    }

    /// A dry biome still thickens, at half strength.
    #[test]
    fn a_biome_without_precipitation_gets_half() {
        assert_eq!(RainFog::target(1.0, 15, false), 0.5);
        assert_eq!(RainFog::target(1.0, 15, true), 1.0);
    }

    /// The multiplier EASES rather than snapping — that is why it is state.
    #[test]
    fn the_multiplier_eases_toward_its_target() {
        let mut f = RainFog::default();
        f.update(1.0, 15, true, 1.0);
        assert!(
            (f.multiplier() - 0.2).abs() < 1e-6,
            "one tick moves 20% of the way: {}",
            f.multiplier()
        );
        for _ in 0..40 {
            f.update(1.0, 15, true, 1.0);
        }
        assert!(f.multiplier() > 0.99, "and converges: {}", f.multiplier());
    }

    /// It eases back out too, so stepping into shelter clears gradually.
    #[test]
    fn it_eases_back_out_when_the_rain_stops() {
        let mut f = RainFog::default();
        f.converge(1.0, 15, true);
        assert_eq!(f.multiplier(), 1.0);
        f.update(0.0, 15, true, 1.0);
        assert!((f.multiplier() - 0.8).abs() < 1e-6, "{}", f.multiplier());
    }

    /// A large delta must not overshoot past the target.
    #[test]
    fn a_long_frame_does_not_overshoot() {
        let mut f = RainFog::default();
        f.update(1.0, 15, true, 100.0);
        assert_eq!(f.multiplier(), 1.0);
    }

    /// The band closes in, and the end is floored at `min(96, end)` rather
    /// than being allowed to collapse to nothing.
    #[test]
    fn the_band_closes_in_and_the_end_is_floored() {
        let mut f = RainFog::default();
        f.converge(1.0, 15, true);
        // Vanilla's own defaults: 0 and 1024.
        let (s, e) = f.apply(0.0, 1024.0);
        assert_eq!((s, e), (-160.0, 768.0));
        // A band already tighter than the floor keeps its own end.
        let (s2, e2) = f.apply(80.0, 180.0);
        assert_eq!((s2, e2), (-80.0, 96.0));
        // And one already inside the floor is left there, not pushed below it.
        let (_, e3) = f.apply(10.0, 50.0);
        assert_eq!(e3, 50.0, "min(96, 50) is the floor here");
    }

    #[test]
    fn clear_weather_leaves_the_band_alone() {
        let f = RainFog::default();
        assert_eq!(f.apply(80.0, 180.0), (80.0, 180.0));
    }
}
