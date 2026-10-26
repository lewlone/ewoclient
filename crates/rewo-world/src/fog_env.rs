//! M422 — vanilla's fog environments (`net.minecraft.client.renderer.fog`).
//!
//! `FogRenderer.setupFog` walks `FOG_ENVIRONMENTS` in order — lava, powder
//! snow, blindness, darkness, water, atmospheric — and the first that
//! `isApplicable` sets the environmental band (and the sky and cloud fog
//! ends). `computeFogColor` takes its colour from the first applicable
//! environment that `providesColor` (the two mob-effect ones do not), darkens
//! it by the first that `modifiesDarkness` (the same two), and brightens it by
//! the water vision under water or night vision elsewhere.
//!
//! The atmospheric environment is the one Rewo already had (the biome fog
//! colour, the rain band): [`environment_fog`] answers `None` for it, and the
//! caller keeps its band and colour. Everything here is a pure function of
//! its inputs, so the app's per-frame step is a thin adapter.
//!
//! Not modelled: the void darkness (`voidDarknessOnsetRange`, a level-data
//! value the client is not sent), `darkenWorldAmount` (the boss bar's world
//! darkening), a boat's exception in `updateFluidOnEyes`, and the dimension
//! types' own water fog attributes (none of the vanilla four sets one).

use crate::physics::{fluid_top, PhysicsWorld};
use rewo_data::block_physics::Stuck;

/// `LavaFogEnvironment.COLOR`, ARGB.
pub const LAVA_FOG_COLOR: i32 = 0xFF99_1900_u32 as i32;
/// `PowderedSnowFogEnvironment.COLOR`, ARGB.
pub const POWDER_SNOW_FOG_COLOR: i32 = 0xFF9F_BBCC_u32 as i32;
/// `EnvironmentAttributes.WATER_FOG_COLOR`'s default, ARGB.
pub const DEFAULT_WATER_FOG_COLOR: i32 = 0xFF05_0533_u32 as i32;
/// `EnvironmentAttributes.WATER_FOG_START_DISTANCE`'s default. No vanilla
/// biome or dimension sets it.
pub const WATER_FOG_START: f32 = -8.0;
/// `EnvironmentAttributes.WATER_FOG_END_DISTANCE`'s default (the swamps
/// multiply it by 0.85).
pub const WATER_FOG_END: f32 = 96.0;
/// `Projection.zNear` — the near plane `Camera.getNearPlane` probes.
pub const Z_NEAR: f64 = 0.05;

/// `FogType`, with `NONE` already folded into `ATMOSPHERIC`
/// (`FogRenderer.getFogType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FogType {
    Atmospheric,
    Water,
    Lava,
    PowderSnow,
}

/// One float environment attribute's value as a biome writes it: a bare
/// value (an override) or `{argument, modifier}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatModifier {
    Override(f32),
    Add(f32),
    Subtract(f32),
    Multiply(f32),
    Minimum(f32),
    Maximum(f32),
}

impl FloatModifier {
    /// `AttributeModifier.apply(base)` for a float attribute.
    pub fn apply(self, base: f32) -> f32 {
        match self {
            FloatModifier::Override(v) => v,
            FloatModifier::Add(v) => base + v,
            FloatModifier::Subtract(v) => base - v,
            FloatModifier::Multiply(v) => base * v,
            FloatModifier::Minimum(v) => base.min(v),
            FloatModifier::Maximum(v) => base.max(v),
        }
    }
}

/// `Camera.getFluidInCamera`. Water is tested at the eye's own block (the
/// eye below the fluid's height); lava and powder snow at the five near-plane
/// probes `getNearPlane` gives (the forward point and the four corners,
/// `forward·zNear ± up·h ± left·w` with `h = tan(fov/2)·zNear`, `w = h ·
/// aspect`). `forward`, `left` and `up` are the camera's unit axes.
pub fn camera_fog_type(
    world: &dyn PhysicsWorld,
    eye: [f64; 3],
    forward: [f64; 3],
    left: [f64; 3],
    up: [f64; 3],
    fov_degrees: f32,
    aspect: f64,
) -> FogType {
    let block = |p: [f64; 3]| {
        (
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        )
    };
    let (bx, by, bz) = block(eye);
    if let Some((false, top)) = fluid_top(world, bx, by, bz) {
        if eye[1] < top {
            return FogType::Water;
        }
    }
    let h = (f64::from(fov_degrees).to_radians() / 2.0).tan() * Z_NEAR;
    let w = h * aspect;
    let f = forward.map(|c| c * Z_NEAR);
    let probe = |su: f64, sl: f64| -> [f64; 3] {
        std::array::from_fn(|i| f[i] + up[i] * h * su + left[i] * w * sl)
    };
    for p in [
        probe(0.0, 0.0),
        probe(1.0, 1.0),
        probe(1.0, -1.0),
        probe(-1.0, 1.0),
        probe(-1.0, -1.0),
    ] {
        let at = [eye[0] + p[0], eye[1] + p[1], eye[2] + p[2]];
        let (x, y, z) = block(at);
        match fluid_top(world, x, y, z) {
            Some((true, top)) => {
                if at[1] <= top {
                    return FogType::Lava;
                }
            }
            _ => {
                if world.block(x, y, z).stuck == Stuck::PowderSnow {
                    return FogType::PowderSnow;
                }
            }
        }
    }
    FogType::Atmospheric
}

/// `Entity.updateFluidOnEyes`' water half: the block at `eye − 0.11111111`
/// holds water whose height is above that point.
pub fn water_on_eyes(world: &dyn PhysicsWorld, x: f64, eye_y: f64, z: f64) -> bool {
    let y = eye_y - f64::from(0.111_111_11f32);
    let (bx, by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    matches!(fluid_top(world, bx, by, bz), Some((false, top)) if top > y)
}

/// `LocalPlayer.waterVisionTime`: up 1 a tick with the eyes under water (10
/// for a spectator), else down 10, within `0..=600`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaterVision {
    time: i32,
    eyes_in_water: bool,
}

impl WaterVision {
    /// One `aiStep`, with this tick's `isEyeInFluid(WATER)`.
    pub fn tick(&mut self, eyes_in_water: bool, spectator: bool) {
        self.eyes_in_water = eyes_in_water;
        if eyes_in_water {
            let speed = if spectator { 10 } else { 1 };
            self.time = (self.time + speed).clamp(0, 600);
        } else if self.time > 0 {
            self.time = (self.time - 10).clamp(0, 600);
        }
    }

    /// `getWaterVision()`: 0 with the eyes out of water; else 0.6 over the
    /// first 100 ticks and 0.4 more over the next 500.
    pub fn value(&self) -> f32 {
        if !self.eyes_in_water {
            return 0.0;
        }
        let t = self.time as f32;
        if t >= 600.0 {
            return 1.0;
        }
        let a = (t / 100.0).clamp(0.0, 1.0);
        let b = if t < 100.0 {
            0.0
        } else {
            ((t - 100.0) / 500.0).clamp(0.0, 1.0)
        };
        a * 0.6 + b * 0.399_999_98
    }
}

/// A mob effect a fog environment reads, as the local player has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectState {
    /// `isInfiniteDuration()`.
    pub infinite: bool,
    /// Ticks left.
    pub duration: i32,
}

impl EffectState {
    /// `endsWithin(ticks)`.
    fn ends_within(self, ticks: i32) -> bool {
        !self.infinite && self.duration <= ticks
    }
}

/// What the fog environments read off the camera's entity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FogInputs {
    pub fog_type: Option<FogType>,
    pub spectator: bool,
    pub fire_resistance: bool,
    pub blindness: Option<EffectState>,
    /// `getEffectBlendFactor(DARKNESS)` with darkness present, else `None`.
    pub darkness: Option<f32>,
    /// `getWaterVision()`.
    pub water_vision: f32,
    /// `WATER_FOG_START_DISTANCE` / `WATER_FOG_END_DISTANCE` at the camera.
    pub water_fog: [f32; 2],
    /// The render distance in blocks (`renderDistanceInChunks · 16`).
    pub render_distance: f32,
}

impl FogInputs {
    fn fog_type(&self) -> FogType {
        self.fog_type.unwrap_or(FogType::Atmospheric)
    }
}

/// The band an environment sets: `environmentalStart` / `End`, `skyEnd`,
/// `cloudEnd`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentFog {
    pub start: f32,
    pub end: f32,
    pub sky_end: f32,
    pub cloud_end: f32,
}

/// `setupFog`'s environment loop: the first applicable of lava, powder snow,
/// blindness, darkness and water, or `None` when only the atmospheric one
/// applies (the caller keeps that band).
pub fn environment_fog(i: &FogInputs) -> Option<EnvironmentFog> {
    let rd = i.render_distance;
    let same = |start: f32, end: f32| EnvironmentFog {
        start,
        end,
        sky_end: end,
        cloud_end: end,
    };
    match i.fog_type() {
        FogType::Lava => {
            return Some(if i.spectator {
                same(-8.0, rd * 0.5)
            } else if i.fire_resistance {
                same(0.0, 5.0)
            } else {
                same(0.25, 1.0)
            });
        }
        FogType::PowderSnow => {
            return Some(if i.spectator {
                same(-8.0, rd * 0.5)
            } else {
                same(0.0, 2.0)
            });
        }
        _ => {}
    }
    if let Some(b) = i.blindness {
        let distance = if b.infinite {
            5.0
        } else {
            lerp((b.duration as f32 / 20.0).min(1.0), rd, 5.0)
        };
        return Some(EnvironmentFog {
            start: distance * 0.25,
            end: distance,
            sky_end: distance * 0.8,
            cloud_end: distance * 0.8,
        });
    }
    if let Some(blend) = i.darkness {
        let distance = lerp(blend, rd, 15.0);
        return Some(EnvironmentFog {
            start: distance * 0.75,
            ..same(distance, distance)
        });
    }
    if i.fog_type() == FogType::Water {
        let end = i.water_fog[1] * i.water_vision.max(0.25);
        return Some(same(i.water_fog[0], end));
    }
    None
}

/// Where `computeFogColor` takes its base colour from: the first applicable
/// environment that provides one (the mob-effect ones do not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSource {
    Lava,
    PowderSnow,
    Water,
    Atmospheric,
}

pub fn color_source(fog_type: FogType) -> ColorSource {
    match fog_type {
        FogType::Lava => ColorSource::Lava,
        FogType::PowderSnow => ColorSource::PowderSnow,
        FogType::Water => ColorSource::Water,
        FogType::Atmospheric => ColorSource::Atmospheric,
    }
}

/// `computeFogColor`'s darkness: blindness's `getModifiedDarkness` (1, or
/// the last second's `duration / 20`) or darkness's (its blend factor),
/// whichever environment applies first, over a starting 0 (the void darkness
/// is not modelled).
pub fn fog_darkness(i: &FogInputs) -> f32 {
    if let Some(b) = i.blindness {
        return if b.ends_within(19) {
            (b.duration as f32 / 20.0).max(0.0)
        } else {
            1.0
        };
    }
    i.darkness.map_or(0.0, |blend| blend.max(0.0))
}

/// The tail of `computeFogColor` over a base colour (sRGB floats, the
/// `ARGB.redFloat` values): the darkness square (not in lava or powder
/// snow), then the brighten toward the brightest channel by `brighten` (the
/// water vision under water; else night vision's scale, `0` without it or
/// with darkness).
pub fn finish_fog_color(
    base: [f32; 3],
    fog_type: FogType,
    darkness: f32,
    brighten: f32,
) -> [f32; 3] {
    let [mut r, mut g, mut b] = base;
    if darkness > 0.0 && fog_type != FogType::Lava && fog_type != FogType::PowderSnow {
        let brightness = (1.0 - darkness) * (1.0 - darkness);
        r *= brightness;
        g *= brightness;
        b *= brightness;
    }
    if r != 0.0 && g != 0.0 && b != 0.0 {
        let max = r.max(g.max(b));
        let target = 1.0 / max.clamp(0.07, 1.0);
        let target_max = 1.0 / max;
        let scale = |c: f32| if max != c { target } else { target_max };
        let (sr, sg, sb) = (scale(r), scale(g), scale(b));
        r = lerp(brighten, r, r * sr);
        g = lerp(brighten, g, g * sg);
        b = lerp(brighten, b, b * sb);
    }
    [r, g, b]
}

/// What a frame's fog becomes under the environments: the band (`None`:
/// the atmospheric band stands) and the colour as sRGB floats (`None`: the
/// atmospheric colour stands, untouched).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogPlan {
    pub band: Option<EnvironmentFog>,
    pub color: Option<[f32; 3]>,
}

/// `setupFog` and `computeFogColor` together, over the colours they read:
/// `night_vision` is `GameRenderer.nightVisionScale` (0 without the effect),
/// `water_fog_color` the camera's `WATER_FOG_COLOR`, `atmospheric` the
/// colour the frame's atmospheric fog already has (sRGB floats). The
/// brighten is the water vision under water, else night vision unless
/// darkness is present.
pub fn plan(
    i: &FogInputs,
    night_vision: f32,
    water_fog_color: i32,
    atmospheric: [f32; 3],
) -> FogPlan {
    let fog_type = i.fog_type();
    let darkness = fog_darkness(i);
    let brighten = if fog_type == FogType::Water {
        i.water_vision
    } else if i.darkness.is_none() {
        night_vision
    } else {
        0.0
    };
    let source = color_source(fog_type);
    let color = if source == ColorSource::Atmospheric && darkness <= 0.0 && brighten <= 0.0 {
        None
    } else {
        let base = match source {
            ColorSource::Lava => argb_floats(LAVA_FOG_COLOR),
            ColorSource::PowderSnow => argb_floats(POWDER_SNOW_FOG_COLOR),
            ColorSource::Water => argb_floats(water_fog_color),
            ColorSource::Atmospheric => atmospheric,
        };
        Some(finish_fog_color(base, fog_type, darkness, brighten))
    };
    FogPlan {
        band: environment_fog(i),
        color,
    }
}

/// The ARGB colour as `ARGB.red/green/blueFloat`.
pub fn argb_floats(c: i32) -> [f32; 3] {
    [
        ((c >> 16) & 0xFF) as f32 / 255.0,
        ((c >> 8) & 0xFF) as f32 / 255.0,
        (c & 0xFF) as f32 / 255.0,
    ]
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_data::block_physics::{BlockPhysics, PhysFluid};

    /// A world of one column of blocks keyed by y, everything else air.
    struct Column(Vec<(i32, BlockPhysics)>);

    impl PhysicsWorld for Column {
        fn collision(&self, _: i32, _: i32, _: i32) -> &[[f32; 6]] {
            &[]
        }
        fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
            if (x, z) != (0, 0) {
                return BlockPhysics::AIR;
            }
            self.0
                .iter()
                .find(|(by, _)| *by == y)
                .map_or(BlockPhysics::AIR, |(_, b)| *b)
        }
        fn has_chunk(&self, _: i32, _: i32) -> bool {
            true
        }
        fn min_y(&self) -> i32 {
            -64
        }
    }

    fn water(own: f32) -> BlockPhysics {
        BlockPhysics {
            fluid: PhysFluid::Water { own_height: own },
            ..BlockPhysics::AIR
        }
    }
    fn lava(own: f32) -> BlockPhysics {
        BlockPhysics {
            fluid: PhysFluid::Lava { own_height: own },
            ..BlockPhysics::AIR
        }
    }
    fn snow() -> BlockPhysics {
        BlockPhysics {
            stuck: Stuck::PowderSnow,
            ..BlockPhysics::default()
        }
    }
    const FWD: [f64; 3] = [0.0, 0.0, 1.0];
    const LEFT: [f64; 3] = [1.0, 0.0, 0.0];
    const UP: [f64; 3] = [0.0, 1.0, 0.0];
    fn kind(w: &Column, eye: [f64; 3]) -> FogType {
        camera_fog_type(w, eye, FWD, LEFT, UP, 70.0, 16.0 / 9.0)
    }

    /// `getFluidInCamera`: water under its surface at the eye's block (a
    /// source's 8/9, full with water above), lava at the near plane under
    /// its own, powder snow anywhere in the probed block, else atmospheric.
    #[test]
    fn the_camera_fluid_follows_get_fluid_in_camera() {
        let pool = Column(vec![(64, water(8.0 / 9.0))]);
        assert_eq!(kind(&pool, [0.5, 64.5, 0.5]), FogType::Water);
        assert_eq!(kind(&pool, [0.5, 64.95, 0.5]), FogType::Atmospheric);
        let deep = Column(vec![(64, water(8.0 / 9.0)), (65, water(8.0 / 9.0))]);
        assert_eq!(kind(&deep, [0.5, 64.95, 0.5]), FogType::Water);
        let pit = Column(vec![(64, lava(8.0 / 9.0))]);
        assert_eq!(kind(&pit, [0.5, 64.5, 0.5]), FogType::Lava);
        assert_eq!(kind(&pit, [0.5, 64.95, 0.5]), FogType::Atmospheric);
        let drift = Column(vec![(64, snow())]);
        assert_eq!(kind(&drift, [0.5, 64.5, 0.5]), FogType::PowderSnow);
        assert_eq!(
            kind(&Column(vec![]), [0.5, 64.5, 0.5]),
            FogType::Atmospheric
        );
        // The probes reach 5 cm forward: an eye 3 cm short of a lava block
        // ahead already sees lava.
        let ahead = Column(vec![]);
        let wall = |x: i32, y: i32, z: i32| (x, y, z) == (0, 64, 1);
        struct Ahead<'a>(&'a Column, &'a dyn Fn(i32, i32, i32) -> bool);
        impl PhysicsWorld for Ahead<'_> {
            fn collision(&self, _: i32, _: i32, _: i32) -> &[[f32; 6]] {
                &[]
            }
            fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
                if (self.1)(x, y, z) {
                    lava(8.0 / 9.0)
                } else {
                    self.0.block(x, y, z)
                }
            }
            fn has_chunk(&self, _: i32, _: i32) -> bool {
                true
            }
            fn min_y(&self) -> i32 {
                -64
            }
        }
        let w = Ahead(&ahead, &wall);
        let eye = [0.5, 64.5, 0.97];
        assert_eq!(
            camera_fog_type(&w, eye, FWD, LEFT, UP, 70.0, 1.0),
            FogType::Lava
        );
    }

    /// `updateFluidOnEyes` measures 0.11111111 below the eye.
    #[test]
    fn the_eyes_are_in_water_below_the_surface_by_a_ninth() {
        let pool = Column(vec![(64, water(8.0 / 9.0))]);
        assert!(water_on_eyes(&pool, 0.5, 64.9, 0.5));
        assert!(!water_on_eyes(&pool, 0.5, 65.01, 0.5));
    }

    /// `getWaterVision`: 0.6 at 100 ticks, 1 at 600, falling 10 a tick out
    /// of water, and 0 the moment the eyes leave.
    #[test]
    fn water_vision_ramps_as_vanilla() {
        let mut v = WaterVision::default();
        for _ in 0..100 {
            v.tick(true, false);
        }
        assert!((v.value() - 0.6).abs() < 1e-6, "{}", v.value());
        for _ in 0..500 {
            v.tick(true, false);
        }
        assert_eq!(v.value(), 1.0);
        v.tick(false, false);
        assert_eq!(v.value(), 0.0);
        assert_eq!(v.time, 590);
        let mut s = WaterVision::default();
        s.tick(true, true);
        assert_eq!(s.time, 10);
    }

    /// `setupFog`'s order and numbers: lava over everything (0.25..1, 0..5
    /// fire-resistant, −8..rd/2 spectating), powder snow (0..2), blindness
    /// (5 blocks a second before its end), darkness (rd toward 15 by the
    /// blend), water (−8..96 × max(0.25, vision)), else atmospheric.
    #[test]
    fn the_environments_apply_in_setup_fog_order() {
        let base = FogInputs {
            render_distance: 192.0,
            water_fog: [WATER_FOG_START, WATER_FOG_END],
            ..Default::default()
        };
        let band = |i: FogInputs| environment_fog(&i).map(|e| (e.start, e.end, e.sky_end));
        assert_eq!(band(base), None);
        let lava_in = FogInputs {
            fog_type: Some(FogType::Lava),
            blindness: Some(EffectState {
                infinite: true,
                duration: -1,
            }),
            ..base
        };
        assert_eq!(band(lava_in), Some((0.25, 1.0, 1.0)));
        assert_eq!(
            band(FogInputs {
                fire_resistance: true,
                ..lava_in
            }),
            Some((0.0, 5.0, 5.0))
        );
        assert_eq!(
            band(FogInputs {
                spectator: true,
                ..lava_in
            }),
            Some((-8.0, 96.0, 96.0))
        );
        let snow_in = FogInputs {
            fog_type: Some(FogType::PowderSnow),
            ..base
        };
        assert_eq!(band(snow_in), Some((0.0, 2.0, 2.0)));
        // Blindness wins over water and darkness.
        let blind = FogInputs {
            fog_type: Some(FogType::Water),
            blindness: Some(EffectState {
                infinite: false,
                duration: 100,
            }),
            darkness: Some(1.0),
            ..base
        };
        assert_eq!(band(blind), Some((1.25, 5.0, 4.0)));
        // Ten ticks before its end it is halfway back to the render distance.
        let ending = FogInputs {
            blindness: Some(EffectState {
                infinite: false,
                duration: 10,
            }),
            ..blind
        };
        assert_eq!(band(ending).map(|b| b.1), Some(98.5));
        let dark = FogInputs {
            darkness: Some(0.5),
            ..base
        };
        assert_eq!(band(dark), Some((77.625, 103.5, 103.5)));
        let under = FogInputs {
            fog_type: Some(FogType::Water),
            water_vision: 0.1,
            ..base
        };
        assert_eq!(band(under), Some((-8.0, 24.0, 24.0)));
        assert_eq!(
            band(FogInputs {
                water_vision: 1.0,
                ..under
            })
            .map(|b| b.1),
            Some(96.0)
        );
    }

    /// The plan: plain air changes nothing; under water the water fog's
    /// colour, brightened by the vision, with its band; blinded in the open
    /// the atmospheric colour goes black; in lava the colour is lava's
    /// whatever the darkness.
    #[test]
    fn the_plan_takes_each_environments_band_and_colour() {
        let base = FogInputs {
            render_distance: 192.0,
            water_fog: [WATER_FOG_START, WATER_FOG_END],
            ..Default::default()
        };
        let sky = [0.6, 0.7, 0.9];
        assert_eq!(
            plan(&base, 0.0, DEFAULT_WATER_FOG_COLOR, sky),
            FogPlan {
                band: None,
                color: None
            }
        );
        let under = FogInputs {
            fog_type: Some(FogType::Water),
            water_vision: 0.0,
            ..base
        };
        let p = plan(&under, 0.0, 0xFF3F_76E4_u32 as i32, sky);
        assert_eq!(p.band.map(|b| (b.start, b.end)), Some((-8.0, 24.0)));
        assert_eq!(p.color, Some(argb_floats(0xFF3F_76E4_u32 as i32)));
        let seeing = plan(
            &FogInputs {
                water_vision: 1.0,
                ..under
            },
            0.0,
            0xFF3F_76E4_u32 as i32,
            sky,
        );
        let c = seeing.color.unwrap();
        assert!(
            (c[2] - 1.0).abs() < 1e-6,
            "brightened to the blue channel: {c:?}"
        );
        let blind = FogInputs {
            blindness: Some(EffectState {
                infinite: true,
                duration: -1,
            }),
            ..base
        };
        assert_eq!(
            plan(&blind, 0.0, DEFAULT_WATER_FOG_COLOR, sky).color,
            Some([0.0; 3])
        );
        let lava_blind = FogInputs {
            fog_type: Some(FogType::Lava),
            ..blind
        };
        assert_eq!(
            plan(&lava_blind, 0.0, DEFAULT_WATER_FOG_COLOR, sky).color,
            Some(argb_floats(LAVA_FOG_COLOR))
        );
        // Night vision brightens the open air too, unless darkness is on.
        assert!(plan(&base, 1.0, DEFAULT_WATER_FOG_COLOR, sky)
            .color
            .is_some());
        let dark = FogInputs {
            darkness: Some(0.0),
            ..base
        };
        assert_eq!(plan(&dark, 1.0, DEFAULT_WATER_FOG_COLOR, sky).color, None);
    }

    /// `computeFogColor`'s tail: blindness blacks the fog out (but not in
    /// lava), its last second lifts it, and the brighten scales toward the
    /// brightest channel (×1/max for it, ×1/clamp(max, 0.07, 1) the rest).
    #[test]
    fn the_fog_colour_darkens_and_brightens_as_vanilla() {
        let blind = FogInputs {
            blindness: Some(EffectState {
                infinite: true,
                duration: -1,
            }),
            ..Default::default()
        };
        assert_eq!(fog_darkness(&blind), 1.0);
        let base = [0.5, 0.25, 0.1];
        assert_eq!(
            finish_fog_color(base, FogType::Atmospheric, 1.0, 0.0),
            [0.0, 0.0, 0.0]
        );
        assert_eq!(finish_fog_color(base, FogType::Lava, 1.0, 0.0), base);
        let last = FogInputs {
            blindness: Some(EffectState {
                infinite: false,
                duration: 10,
            }),
            ..Default::default()
        };
        assert_eq!(fog_darkness(&last), 0.5);
        let bright = finish_fog_color(base, FogType::Water, 0.0, 1.0);
        assert!(
            (bright[0] - 1.0).abs() < 1e-6 && (bright[1] - 0.5).abs() < 1e-6,
            "{bright:?}"
        );
        assert_eq!(
            argb_floats(LAVA_FOG_COLOR),
            [0x99 as f32 / 255.0, 0x19 as f32 / 255.0, 0.0]
        );
        assert_eq!(
            FloatModifier::Multiply(0.85).apply(WATER_FOG_END),
            96.0 * 0.85
        );
    }
}
