//! Per-biome color — a from-the-decompile port of vanilla's biome tint stack.
//!
//! Ground truth (26.2 Mojmap):
//! - `BiomeManager.getBiome` — the fiddled block→biome lookup (block tint).
//! - `ClientLevel.calculateBlockTint` — the `biomeBlendRadius` (default 2)
//!   `(2r+1)²` same-Y average, integer channel mean.
//! - `Biome.getGrassColor` / `getFoliageColor` / `getDryFoliageColor` /
//!   `getWaterColor` + `BiomeSpecialEffects.GrassColorModifier`.
//! - `ColorMapColorUtil.get` — the colormap clamp/index.
//! - `ARGB.srgbLerp` — the integer-channel color lerp used by the sky/fog
//!   spatial interpolation (`LerpFunction.ofColor`).
//! - `EnvironmentAttributeProbe` + `GaussianSampler` +
//!   `SpatialAttributeInterpolator` — the DIFFERENT camera sky/fog path (raw
//!   quart samples, not the fiddle; 6³ Gaussian; dimension base then biome
//!   positional override).
//!
//! This crate is pure: colormap pixels arrive as raw `Vec<i32>` (decoded by
//! `rewo-data`), and quart→biome sampling arrives as a closure the caller wires
//! to the chunk biome containers. So the whole file is unit-testable headlessly.

use std::sync::Arc;

use crate::biome_noise::BiomeInfoNoise;

// -- ARGB helpers (verbatim `net.minecraft.util.ARGB`) -----------------------

#[inline]
pub fn argb_red(c: i32) -> i32 {
    (c >> 16) & 0xFF
}
#[inline]
pub fn argb_green(c: i32) -> i32 {
    (c >> 8) & 0xFF
}
#[inline]
pub fn argb_blue(c: i32) -> i32 {
    c & 0xFF
}
#[inline]
pub fn argb_alpha(c: i32) -> i32 {
    ((c as u32) >> 24) as i32
}
#[inline]
pub fn argb(a: i32, r: i32, g: i32, b: i32) -> i32 {
    ((a & 0xFF) << 24) | ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF)
}
#[inline]
pub fn argb_rgb(r: i32, g: i32, b: i32) -> i32 {
    argb(255, r, g, b)
}
#[inline]
fn argb_opaque(c: i32) -> i32 {
    // `c | 0xFF000000` in Java int arithmetic.
    (c as u32 | 0xFF00_0000) as i32
}

/// `Mth.lerpInt(alpha, p0, p1) = p0 + floor(alpha * (p1 - p0))`.
#[inline]
fn lerp_int(alpha: f32, p0: i32, p1: i32) -> i32 {
    p0 + (alpha * (p1 - p0) as f32).floor() as i32
}

/// `ARGB.srgbLerp` — per-channel `lerpInt`, alpha included.
pub fn srgb_lerp(alpha: f32, p0: i32, p1: i32) -> i32 {
    argb(
        lerp_int(alpha, argb_alpha(p0), argb_alpha(p1)),
        lerp_int(alpha, argb_red(p0), argb_red(p1)),
        lerp_int(alpha, argb_green(p0), argb_green(p1)),
        lerp_int(alpha, argb_blue(p0), argb_blue(p1)),
    )
}

// -- colormap ----------------------------------------------------------------

/// Vanilla colormap defaults (the `defaultMapColor` fed to `ColorMapColorUtil`).
pub const GRASS_DEFAULT: i32 = -65281; // 0xFFFF00FF
pub const FOLIAGE_DEFAULT: i32 = -12012264; // FoliageColor.FOLIAGE_DEFAULT
pub const DRY_FOLIAGE_DEFAULT: i32 = -10732494; // DryFoliageColor.FOLIAGE_DRY_DEFAULT

/// `ColorMapColorUtil.get(temp, rain, pixels, default)`.
///
/// `temp`/`rain` are the already-clamped [0,1] climate values. The `(int)`
/// casts truncate toward zero (matched by `as i32` on non-negative products).
pub fn colormap_get(temp: f64, rain_in: f64, pixels: &[i32], default: i32) -> i32 {
    let rain = rain_in * temp;
    let x = ((1.0 - temp) * 255.0) as i32;
    let y = ((1.0 - rain) * 255.0) as i32;
    let index = ((y << 8) | x) as usize;
    if index >= pixels.len() {
        default
    } else {
        pixels[index]
    }
}

/// The three biome colormaps, 65536 ARGB ints each (`grass.png`/`foliage.png`/
/// `dry_foliage.png` from the client jar). A single-element vec (the default
/// color) is a valid neutral fallback: every lookup falls past the length.
#[derive(Clone)]
pub struct Colormaps {
    pub grass: Arc<Vec<i32>>,
    pub foliage: Arc<Vec<i32>>,
    pub dry_foliage: Arc<Vec<i32>>,
}

impl Colormaps {
    /// A neutral fallback (no colormap textures): every lookup returns the
    /// vanilla default map color. Used when the jar colormaps aren't loaded.
    pub fn neutral() -> Self {
        Self {
            grass: Arc::new(vec![GRASS_DEFAULT]),
            foliage: Arc::new(vec![FOLIAGE_DEFAULT]),
            dry_foliage: Arc::new(vec![DRY_FOLIAGE_DEFAULT]),
        }
    }

    /// Build from decoded colormap pixel arrays (`rewo_data::assets::
    /// colormap_pixels`). An empty array for a channel falls back to that
    /// channel's single default color (every lookup lands past the length).
    pub fn from_pixels(grass: Vec<i32>, foliage: Vec<i32>, dry_foliage: Vec<i32>) -> Self {
        let or_default = |px: Vec<i32>, default: i32| {
            if px.is_empty() {
                Arc::new(vec![default])
            } else {
                Arc::new(px)
            }
        };
        Self {
            grass: or_default(grass, GRASS_DEFAULT),
            foliage: or_default(foliage, FOLIAGE_DEFAULT),
            dry_foliage: or_default(dry_foliage, DRY_FOLIAGE_DEFAULT),
        }
    }
}

// -- biome definitions -------------------------------------------------------

/// `BiomeSpecialEffects.GrassColorModifier`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrassModifier {
    None,
    DarkForest,
    Swamp,
}

impl GrassModifier {
    pub fn parse(s: &str) -> Self {
        match s {
            "dark_forest" => GrassModifier::DarkForest,
            "swamp" => GrassModifier::Swamp,
            _ => GrassModifier::None,
        }
    }

    /// `GrassColorModifier.modifyColor(x, z, baseColor)`.
    pub fn apply(self, x: f64, z: f64, base: i32, noise: &BiomeInfoNoise) -> i32 {
        match self {
            GrassModifier::None => base,
            // `ARGB.opaque((baseColor & 16711422) + 2634762 >> 1)` — Java `+`
            // binds tighter than `>>`, so it's `((base & 0xFEFEFE) + 2634762) >> 1`.
            GrassModifier::DarkForest => argb_opaque(((base & 16711422) + 2634762) >> 1),
            GrassModifier::Swamp => {
                let ground = noise.value(x * 0.0225, z * 0.0225);
                if ground < -0.1 {
                    -11766212
                } else {
                    -9801671
                }
            }
        }
    }
}

/// One biome, in the registry's raw (wire) order. Colors are ARGB ints.
#[derive(Clone, Debug)]
pub struct BiomeDef {
    pub name: String,
    pub temperature: f32,
    pub downfall: f32,
    pub water_color: i32,
    pub grass_override: Option<i32>,
    pub foliage_override: Option<i32>,
    pub dry_foliage_override: Option<i32>,
    pub grass_modifier: GrassModifier,
    /// Positional `visual/sky_color` / `visual/fog_color` overrides (the
    /// bare-value form vanilla uses for these two). `None` = inherit the
    /// dimension base.
    pub sky_color: Option<i32>,
    pub fog_color: Option<i32>,
    /// `has_precipitation` — whether weather falls here at all. M33 reads it;
    /// with `temperature` + `temperature_modifier` it decides rain vs snow vs
    /// nothing (see [`crate::weather`]).
    pub has_precipitation: bool,
    /// `temperature_modifier`. M14 dropped this as colour-irrelevant, which it
    /// is; it is precipitation-relevant, because `FROZEN` pins whole patches
    /// of a biome to 0.2 and turns their rain to snow.
    pub temperature_modifier: crate::weather::TemperatureModifier,
}

impl BiomeDef {
    fn clamped(v: f32) -> f64 {
        v.clamp(0.0, 1.0) as f64
    }

    /// `Biome.getGrassColorFromTexture` / override — the base before the
    /// modifier.
    fn base_grass_color(&self, maps: &Colormaps) -> i32 {
        match self.grass_override {
            Some(c) => c,
            None => colormap_get(
                Self::clamped(self.temperature),
                Self::clamped(self.downfall),
                &maps.grass,
                GRASS_DEFAULT,
            ),
        }
    }

    /// `Biome.getGrassColor(x, z)` = modifier applied to the base.
    pub fn grass_color(&self, x: f64, z: f64, maps: &Colormaps, noise: &BiomeInfoNoise) -> i32 {
        let base = self.base_grass_color(maps);
        self.grass_modifier.apply(x, z, base, noise)
    }

    /// `Biome.getFoliageColor`.
    pub fn foliage_color(&self, maps: &Colormaps) -> i32 {
        self.foliage_override.unwrap_or_else(|| {
            colormap_get(
                Self::clamped(self.temperature),
                Self::clamped(self.downfall),
                &maps.foliage,
                FOLIAGE_DEFAULT,
            )
        })
    }

    /// `Biome.getDryFoliageColor`.
    pub fn dry_foliage_color(&self, maps: &Colormaps) -> i32 {
        self.dry_foliage_override.unwrap_or_else(|| {
            colormap_get(
                Self::clamped(self.temperature),
                Self::clamped(self.downfall),
                &maps.dry_foliage,
                DRY_FOLIAGE_DEFAULT,
            )
        })
    }

    /// `Biome.getWaterColor`.
    pub fn water_color(&self) -> i32 {
        self.water_color
    }
}

/// Which colormap/effect a tinted face samples — mirrors the vanilla
/// `ColorResolver`s in `BiomeColors`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorResolver {
    Grass,
    Foliage,
    DryFoliage,
    Water,
}

// -- registry + context ------------------------------------------------------

/// The dynamic `minecraft:worldgen/biome` registry, in raw wire order, plus the
/// dimension base sky/fog (from the `dimension_type` attributes).
#[derive(Clone, Debug)]
pub struct BiomeRegistry {
    pub biomes: Vec<BiomeDef>,
    /// In-memory global-palette bits for a *direct* biome container:
    /// `ceil(log2(count))` (`PalettedContainer` global strategy).
    pub global_bits: u32,
    /// Dimension base `visual/sky_color` / `visual/fog_color` (the constant
    /// layer the biome positional layer sits on top of).
    pub dimension_sky: Option<i32>,
    pub dimension_fog: Option<i32>,
}

impl BiomeRegistry {
    pub fn new(biomes: Vec<BiomeDef>) -> Self {
        let global_bits = ceil_log2(biomes.len());
        Self {
            biomes,
            global_bits,
            dimension_sky: None,
            dimension_fog: None,
        }
    }

    pub fn get(&self, idx: usize) -> Option<&BiomeDef> {
        self.biomes.get(idx)
    }

    pub fn len(&self) -> usize {
        self.biomes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.biomes.is_empty()
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.biomes.iter().position(|b| b.name == name)
    }
}

/// `ceil(log2(n))` — the global biome-palette width, exactly vanilla's
/// `Mth.ceillog2(size)`: the smallest `k` with `2^k >= n`. So `ceil_log2(1) == 0`,
/// `ceil_log2(2) == 1`, `ceil_log2(66) == 7`. A 0-bit width only arises for a
/// single-biome registry, which never uses a direct container (it uses the
/// single-value palette); a *direct* biome read separately clamps the storage
/// width to `>= 1` (see `palette::Container::read`).
pub fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        return 0;
    }
    usize::BITS - (n - 1).leading_zeros()
}

/// Everything the mesher/camera needs to resolve a biome color, behind one
/// `Arc` so `World::snapshot_3x3` clones it for free.
#[derive(Clone)]
pub struct BiomeContext {
    pub registry: Arc<BiomeRegistry>,
    pub colormaps: Colormaps,
    pub biome_zoom_seed: i64,
    pub noise: Arc<BiomeInfoNoise>,
}

impl BiomeContext {
    pub fn new(registry: Arc<BiomeRegistry>, colormaps: Colormaps, biome_zoom_seed: i64) -> Self {
        Self {
            registry,
            colormaps,
            biome_zoom_seed,
            noise: Arc::new(BiomeInfoNoise::new()),
        }
    }

    /// Resolve one biome's color for a resolver at world (x,z).
    fn resolve(&self, biome_idx: usize, resolver: ColorResolver, x: i32, z: i32) -> i32 {
        let Some(b) = self.registry.get(biome_idx) else {
            return -1;
        };
        match resolver {
            ColorResolver::Grass => b.grass_color(x as f64, z as f64, &self.colormaps, &self.noise),
            ColorResolver::Foliage => b.foliage_color(&self.colormaps),
            ColorResolver::DryFoliage => b.dry_foliage_color(&self.colormaps),
            ColorResolver::Water => b.water_color(),
        }
    }

    /// `ClientLevel.calculateBlockTint` — the `(2·radius+1)²` same-Y average of
    /// the fiddled per-block biome color. `sample_quart` returns the raw
    /// quart-cell biome index (the biome container lookup).
    ///
    /// Returns an opaque `[r,g,b]`.
    pub fn block_tint(
        &self,
        x: i32,
        y: i32,
        z: i32,
        resolver: ColorResolver,
        radius: i32,
        sample_quart: &impl Fn(i32, i32, i32) -> u16,
    ) -> [u8; 3] {
        if radius == 0 {
            let biome = self.get_biome(x, y, z, sample_quart);
            let c = self.resolve(biome as usize, resolver, x, z);
            return [argb_red(c) as u8, argb_green(c) as u8, argb_blue(c) as u8];
        }
        let count = ((radius * 2 + 1) * (radius * 2 + 1)) as i64;
        let (mut sr, mut sg, mut sb) = (0i64, 0i64, 0i64);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let (nx, nz) = (x + dx, z + dz);
                let biome = self.get_biome(nx, y, nz, sample_quart);
                let c = self.resolve(biome as usize, resolver, nx, nz);
                sr += argb_red(c) as i64;
                sg += argb_green(c) as i64;
                sb += argb_blue(c) as i64;
            }
        }
        [(sr / count) as u8, (sg / count) as u8, (sb / count) as u8]
    }

    /// `BiomeManager.getBiome(pos)` — the fiddled block→biome lookup.
    pub fn get_biome(
        &self,
        x: i32,
        y: i32,
        z: i32,
        sample_quart: &impl Fn(i32, i32, i32) -> u16,
    ) -> u16 {
        let seed = self.biome_zoom_seed;
        let abs_x = x - 2;
        let abs_y = y - 2;
        let abs_z = z - 2;
        let parent_x = abs_x >> 2;
        let parent_y = abs_y >> 2;
        let parent_z = abs_z >> 2;
        // `(absX & 3) / 4.0` — Rust `&` on i32 gives 0..3 for negatives too.
        let fract_x = (abs_x & 3) as f64 / 4.0;
        let fract_y = (abs_y & 3) as f64 / 4.0;
        let fract_z = (abs_z & 3) as f64 / 4.0;

        let mut min_i = 0usize;
        let mut min_dist = f64::INFINITY;
        for i in 0..8 {
            let x_even = (i & 4) == 0;
            let y_even = (i & 2) == 0;
            let z_even = (i & 1) == 0;
            let corner_x = if x_even { parent_x } else { parent_x + 1 };
            let corner_y = if y_even { parent_y } else { parent_y + 1 };
            let corner_z = if z_even { parent_z } else { parent_z + 1 };
            let dist_x = if x_even { fract_x } else { fract_x - 1.0 };
            let dist_y = if y_even { fract_y } else { fract_y - 1.0 };
            let dist_z = if z_even { fract_z } else { fract_z - 1.0 };
            let next = fiddled_distance(seed, corner_x, corner_y, corner_z, dist_x, dist_y, dist_z);
            if min_dist > next {
                min_i = i;
                min_dist = next;
            }
        }
        let biome_x = if (min_i & 4) == 0 {
            parent_x
        } else {
            parent_x + 1
        };
        let biome_y = if (min_i & 2) == 0 {
            parent_y
        } else {
            parent_y + 1
        };
        let biome_z = if (min_i & 1) == 0 {
            parent_z
        } else {
            parent_z + 1
        };
        sample_quart(biome_x, biome_y, biome_z)
    }

    /// Camera `visual/sky_color` — `EnvironmentAttributeProbe` Gaussian over
    /// raw quart biomes, dimension base then biome positional override. Returns
    /// an opaque ARGB int (the timeline multiply is applied by the caller).
    pub fn camera_sky(&self, eye: [f64; 3], sample_quart: &impl Fn(i32, i32, i32) -> u16) -> i32 {
        let base = self.registry.dimension_sky.unwrap_or(0);
        self.camera_attr(eye, base, sample_quart, |b| b.sky_color)
    }

    /// Camera `visual/fog_color`.
    pub fn camera_fog(&self, eye: [f64; 3], sample_quart: &impl Fn(i32, i32, i32) -> u16) -> i32 {
        let base = self.registry.dimension_fog.unwrap_or(0);
        self.camera_attr(eye, base, sample_quart, |b| b.fog_color)
    }

    fn camera_attr(
        &self,
        eye: [f64; 3],
        base: i32,
        sample_quart: &impl Fn(i32, i32, i32) -> u16,
        pick: impl Fn(&BiomeDef) -> Option<i32>,
    ) -> i32 {
        // `EnvironmentAttributeProbe.tick`: position.scale(0.25) → GaussianSampler.
        let groups =
            gaussian_biome_weights([eye[0] * 0.25, eye[1] * 0.25, eye[2] * 0.25], |x, y, z| {
                sample_quart(x, y, z)
            });
        // `SpatialAttributeInterpolator.applyAttributeLayer`.
        if groups.is_empty() {
            return base;
        }
        // Each biome contributes its override, or the base if it has none
        // (`applyModifier` returns baseValue for a missing entry).
        let source_value = |biome_id: u16| -> i32 {
            self.registry
                .get(biome_id as usize)
                .and_then(&pick)
                .unwrap_or(base)
        };
        if groups.len() == 1 {
            return source_value(groups[0].0);
        }
        let mut result: Option<i32> = None;
        let mut accum = 0.0f64;
        for (biome_id, weight) in &groups {
            let value = source_value(*biome_id);
            accum += weight;
            match result {
                None => result = Some(value),
                Some(prev) => {
                    let frac = (weight / accum) as f32;
                    result = Some(srgb_lerp(frac, prev, value));
                }
            }
        }
        result.unwrap_or(base)
    }
}

/// `LinearCongruentialGenerator.next(rval, c)`:
/// `rval *= rval * MULTIPLIER + INCREMENT; return rval + c;` (Java `long` wrap).
#[inline]
fn lcg_next(rval: i64, c: i64) -> i64 {
    const MULT: i64 = 6364136223846793005;
    const INC: i64 = 1442695040888963407;
    rval.wrapping_mul(rval.wrapping_mul(MULT).wrapping_add(INC))
        .wrapping_add(c)
}

/// `BiomeManager.getFiddle(rval)`.
#[inline]
fn get_fiddle(rval: i64) -> f64 {
    // `Math.floorMod(rval >> 24, 1024) / 1024.0` — arithmetic shift, floorMod.
    let uniform = (rval >> 24).rem_euclid(1024) as f64 / 1024.0;
    (uniform - 0.5) * 0.9
}

/// `BiomeManager.getFiddledDistance`.
fn fiddled_distance(
    seed: i64,
    x_random: i32,
    y_random: i32,
    z_random: i32,
    distance_x: f64,
    distance_y: f64,
    distance_z: f64,
) -> f64 {
    let (xr, yr, zr) = (x_random as i64, y_random as i64, z_random as i64);
    let mut rval = seed;
    rval = lcg_next(rval, xr);
    rval = lcg_next(rval, yr);
    rval = lcg_next(rval, zr);
    rval = lcg_next(rval, xr);
    rval = lcg_next(rval, yr);
    rval = lcg_next(rval, zr);
    let fiddle_x = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fiddle_y = get_fiddle(rval);
    rval = lcg_next(rval, seed);
    let fiddle_z = get_fiddle(rval);
    let sq = |v: f64| v * v;
    sq(distance_z + fiddle_z) + sq(distance_y + fiddle_y) + sq(distance_x + fiddle_x)
}

/// `GaussianSampler.GAUSSIAN_SAMPLE_KERNEL`.
const GAUSSIAN_KERNEL: [f64; 7] = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];

/// `GaussianSampler.sample` + `SpatialAttributeInterpolator.accumulate`: run the
/// 6³ Gaussian over the raw quart biomes, returning `(biome_id, weight)` groups
/// in first-seen order (vanilla groups by attribute-map identity == biome id,
/// iterated in `Reference2DoubleArrayMap` insertion order).
fn gaussian_biome_weights(
    position: [f64; 3],
    sample_quart: impl Fn(i32, i32, i32) -> u16,
) -> Vec<(u16, f64)> {
    // `position.subtract(0.5, 0.5, 0.5)`.
    let px = position[0] - 0.5;
    let py = position[1] - 0.5;
    let pz = position[2] - 0.5;
    let ix = px.floor() as i32;
    let iy = py.floor() as i32;
    let iz = pz.floor() as i32;
    let rx = px - ix as f64;
    let ry = py - iy as f64;
    let rz = pz - iz as f64;
    // `Mth.lerp(rel, KERNEL[k+1], KERNEL[k])`.
    let weight = |rel: f64, k: usize| {
        GAUSSIAN_KERNEL[k + 1] + rel * (GAUSSIAN_KERNEL[k] - GAUSSIAN_KERNEL[k + 1])
    };

    // Insertion-ordered accumulation (small: at most a handful of biomes).
    let mut order: Vec<(u16, f64)> = Vec::new();
    for z in 0..6 {
        let wz = weight(rz, z);
        let sample_z = iz - 2 + z as i32;
        for x in 0..6 {
            let wx = weight(rx, x);
            let sample_x = ix - 2 + x as i32;
            for y in 0..6 {
                let wy = weight(ry, y);
                let sample_y = iy - 2 + y as i32;
                let w = wx * wy * wz;
                let biome = sample_quart(sample_x, sample_y, sample_z);
                match order.iter_mut().find(|(b, _)| *b == biome) {
                    Some((_, acc)) => *acc += w,
                    None => order.push((biome, w)),
                }
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_biome(def: BiomeDef) -> BiomeContext {
        BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![def])),
            Colormaps::neutral(),
            0,
        )
    }

    fn plains() -> BiomeDef {
        BiomeDef {
            name: "minecraft:plains".into(),
            temperature: 0.8,
            downfall: 0.4,
            water_color: 0x3F76E4 | (0xFF << 24),
            grass_override: None,
            foliage_override: None,
            dry_foliage_override: None,
            grass_modifier: GrassModifier::None,
            sky_color: Some(argb_opaque(0x78A7FF)),
            fog_color: None,
            has_precipitation: true,
            temperature_modifier: crate::weather::TemperatureModifier::None,
        }
    }

    #[test]
    fn ceil_log2_matches_vanilla() {
        // Vanilla `Mth.ceillog2`: smallest k with 2^k >= n. ceillog2(1) == 0.
        assert_eq!(ceil_log2(0), 0);
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(64), 6);
        assert_eq!(ceil_log2(65), 7);
        assert_eq!(ceil_log2(66), 7); // the live 26.2 registry size
    }

    #[test]
    fn dark_forest_formula() {
        // `opaque(((base & 0xFEFEFE) + 2634762) >> 1)`.
        let base = 0x00_7ABF_5B; // a green
        let got = GrassModifier::DarkForest.apply(0.0, 0.0, base, &BiomeInfoNoise::new());
        let expect = argb_opaque(((base & 16711422) + 2634762) >> 1);
        assert_eq!(got, expect);
        assert_eq!(argb_alpha(got), 255);
    }

    #[test]
    fn swamp_modifier_picks_by_noise_threshold() {
        let n = BiomeInfoNoise::new();
        // (-50,200) noise < -0.1 → dark; (1000,1000) >= -0.1 → light.
        let dark = GrassModifier::Swamp.apply(-50.0, 200.0, 0, &n);
        let light = GrassModifier::Swamp.apply(1000.0, 1000.0, 0, &n);
        assert_eq!(dark, -11766212);
        assert_eq!(light, -9801671);
    }

    #[test]
    fn colormap_index_axes() {
        // pixels[y<<8 | x] with x from temp, y from rain*temp.
        let mut px = vec![0i32; 65536];
        // temp=1, rain=1 → rain*=temp=1 → x=(1-1)*255=0, y=0 → index 0.
        px[0] = 0x111111;
        // temp=0.5, rain=1 → rain=0.5 → x=(0.5)*255=127, y=(0.5)*255=127 → (127<<8)|127.
        let idx = (127i32 << 8 | 127) as usize;
        px[idx] = 0x222222;
        assert_eq!(colormap_get(1.0, 1.0, &px, -1), 0x111111);
        assert_eq!(colormap_get(0.5, 1.0, &px, -1), 0x222222);
        // Out of range → default (single-element fallback map).
        assert_eq!(colormap_get(0.5, 0.5, &[0x999999], -1), -1);
    }

    #[test]
    fn radius0_vs_radius2_agree_in_single_biome() {
        let ctx = single_biome(plains());
        let sampler = |_x: i32, _y: i32, _z: i32| 0u16; // one biome everywhere
        let r0 = ctx.block_tint(10, 64, 20, ColorResolver::Water, 0, &sampler);
        let r2 = ctx.block_tint(10, 64, 20, ColorResolver::Water, 2, &sampler);
        // A uniform field averages to the same color.
        assert_eq!(r0, r2);
        // Water = plains water_color 0x3F76E4.
        assert_eq!(r0, [0x3F, 0x76, 0xE4]);
    }

    #[test]
    fn block_tint_averages_two_biomes() {
        // Two biomes with distinct water colors; a 5x5 straddling both averages.
        let a = BiomeDef {
            water_color: argb_rgb(0, 0, 0),
            ..plains()
        };
        let b = BiomeDef {
            name: "minecraft:b".into(),
            water_color: argb_rgb(100, 100, 100),
            ..plains()
        };
        let ctx = BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![a, b])),
            Colormaps::neutral(),
            0,
        );
        // Quart biome by x-sign: negative→0, else→1. The fiddle perturbs which
        // cells fall where, so we only assert the average lands strictly
        // between the two (0 and 100), proving both are sampled + integer-mean.
        let sampler = |x: i32, _y: i32, _z: i32| if x < 0 { 0u16 } else { 1u16 };
        let avg = ctx.block_tint(0, 64, 0, ColorResolver::Water, 2, &sampler);
        assert!(avg[0] > 0 && avg[0] < 100, "avg red {} not between", avg[0]);
    }

    #[test]
    fn fiddle_corner_matches_java() {
        // Ground truth from a temurin-25 port of `BiomeManager.getBiome` — the
        // quart corner the fiddle selects for (seed, x, y, z). The sampler
        // records the corner it is asked for.
        let cases: &[(i64, i32, i32, i32, (i32, i32, i32))] = &[
            (0, 0, 64, 0, (0, 16, -1)),
            (0, -13, 70, 200, (-3, 17, 50)),
            (0, 5, -4, -9, (1, -2, -3)),
            (0, 31, 63, -17, (7, 16, -5)),
            (123456789, 0, 64, 0, (0, 15, -1)),
            (123456789, -13, 70, 200, (-4, 17, 50)),
            (123456789, 5, -4, -9, (1, -1, -3)),
            (-987654321, -13, 70, 200, (-4, 17, 49)),
            (-987654321, 5, -4, -9, (1, -2, -2)),
            (-987654321, 31, 63, -17, (7, 15, -5)),
        ];
        for &(seed, x, y, z, expect) in cases {
            let ctx = BiomeContext::new(
                Arc::new(BiomeRegistry::new(vec![plains()])),
                Colormaps::neutral(),
                seed,
            );
            let captured = std::cell::Cell::new((999, 999, 999));
            let sampler = |qx: i32, qy: i32, qz: i32| {
                captured.set((qx, qy, qz));
                0u16
            };
            ctx.get_biome(x, y, z, &sampler);
            assert_eq!(
                captured.get(),
                expect,
                "fiddle corner for seed={seed} pos=({x},{y},{z})"
            );
        }
    }

    #[test]
    fn get_biome_is_deterministic_and_in_range() {
        let ctx = single_biome(plains());
        let sampler = |x: i32, y: i32, z: i32| ((x ^ y ^ z) & 1) as u16;
        for (x, y, z) in [(0, 64, 0), (-13, 70, 200), (5, -4, -9)] {
            let a = ctx.get_biome(x, y, z, &sampler);
            let b = ctx.get_biome(x, y, z, &sampler);
            assert_eq!(a, b, "fiddle not deterministic at ({x},{y},{z})");
            assert!(a <= 1);
        }
    }

    #[test]
    fn camera_sky_single_biome_is_override() {
        let ctx = single_biome(plains());
        let sampler = |_x: i32, _y: i32, _z: i32| 0u16;
        let sky = ctx.camera_sky([8.0, 64.0, 8.0], &sampler);
        assert_eq!(sky, argb_opaque(0x78A7FF));
    }

    #[test]
    fn camera_sky_missing_override_falls_back_to_dimension_base() {
        let mut reg = BiomeRegistry::new(vec![BiomeDef {
            sky_color: None,
            ..plains()
        }]);
        reg.dimension_sky = Some(argb_opaque(0x0A0B0C));
        let ctx = BiomeContext::new(Arc::new(reg), Colormaps::neutral(), 0);
        let sampler = |_x: i32, _y: i32, _z: i32| 0u16;
        assert_eq!(
            ctx.camera_sky([8.0, 64.0, 8.0], &sampler),
            argb_opaque(0x0A0B0C)
        );
    }

    #[test]
    fn camera_sky_blends_two_biomes_via_srgb_lerp() {
        // Two biomes with distinct sky overrides; a quart field split by x-sign
        // makes the 6³ Gaussian straddle both, so the result is a weighted
        // srgbLerp strictly between them (not either endpoint, not an average of
        // something else).
        let a = BiomeDef {
            sky_color: Some(argb_opaque(0x000000)),
            ..plains()
        };
        let b = BiomeDef {
            name: "minecraft:b".into(),
            sky_color: Some(argb_opaque(0xFFFFFF)),
            ..plains()
        };
        let ctx = BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![a, b])),
            Colormaps::neutral(),
            0,
        );
        // Quart biome by x-sign; the eye sits on the boundary so both appear.
        let sampler = |qx: i32, _qy: i32, _qz: i32| if qx < 0 { 0u16 } else { 1u16 };
        // eye.x * 0.25 near 0 → the 6³ window spans negative + non-negative quarts.
        let sky = ctx.camera_sky([0.0, 64.0, 0.0], &sampler);
        let r = argb_red(sky);
        assert!(
            r > 0 && r < 255,
            "blended sky red {r} should be strictly between 0 and 255"
        );
        // Green/blue blend identically (both endpoints are grayscale).
        assert_eq!(argb_red(sky), argb_green(sky));
        assert_eq!(argb_green(sky), argb_blue(sky));
    }

    #[test]
    fn gaussian_groups_single_biome_to_exact_override() {
        // A uniform quart field → one group → the override verbatim (no lerp
        // rounding drift). Confirms the size==1 fast path.
        let ctx = single_biome(BiomeDef {
            sky_color: Some(argb_opaque(0x123456)),
            ..plains()
        });
        let sampler = |_x: i32, _y: i32, _z: i32| 0u16;
        assert_eq!(
            ctx.camera_sky([100.5, 64.0, -33.25], &sampler),
            argb_opaque(0x123456)
        );
    }

    #[test]
    fn srgb_lerp_is_integer_floor() {
        // 50% between 0 and 255 → floor(0.5*255) = 127.
        assert_eq!(
            argb_red(srgb_lerp(0.5, argb_rgb(0, 0, 0), argb_rgb(255, 0, 0))),
            127
        );
        // endpoints exact.
        assert_eq!(srgb_lerp(0.0, 0x10, 0x20), 0x10);
        assert_eq!(srgb_lerp(1.0, 0x10, 0x20), 0x20);
    }
}
