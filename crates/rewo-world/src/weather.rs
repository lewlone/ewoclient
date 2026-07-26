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
/// Rain scales red and green by `1 - rain*0.5` but blue only by `1 - rain*0.4`,
/// so a rainy sky does not merely dim — it goes **bluer** as it dims. Thunder
/// then scales all three equally. Alpha is untouched.
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
