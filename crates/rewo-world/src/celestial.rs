//! Clear-weather Overworld celestial state — sun/moon/star angles, star
//! brightness, the sunrise/sunset colour, and the moon phase.
//!
//! Everything here is transcribed from `Timelines.OVERWORLD_DAY` (the
//! attribute timeline that replaced the old hard-coded `getSkyDarken`) and the
//! machinery it drives: `KeyframeTrackSampler`, `EasingType.CubicBezier`, and
//! the per-modifier `LerpFunction`. The point of porting the sampler faithfully
//! rather than eyeballing a curve is that the SUN/MOON/STAR angle tracks are
//! *not* a naive lerp — each carries the symmetric cubic-bezier ease
//! `symmetricCubicBezier(0.362, 0.241)` and, critically, its two keyframes both
//! sit at tick 6000 (values 360 and 0). `KeyframeTrackSampler.bakeSegments`
//! turns that into a wrap-around pair of segments spanning
//! `[6000 - 24000 .. 6000]` and `[6000 .. 6000 + 24000]`; the bezier-eased lerp
//! across those is what sweeps the sun once per day. A degrees-wrapping lerp
//! (the `partialTickLerp` for `ANGLE_DEGREES`) would freeze it — the timeline
//! uses the *keyframe* lerp, which for `ANGLE_DEGREES` is plain linear
//! (`AttributeType.keyframeLerp` is the first `LerpFunction`, `ofFloat()`).
//!
//! Ground truth (26.2 decompile), never a wiki:
//! - `net.minecraft.world.timeline.Timelines`
//! - `net.minecraft.world.timeline.{KeyframeTrackSampler is util}` /
//!   `net.minecraft.util.KeyframeTrackSampler` + `KeyframeTrack`
//! - `net.minecraft.util.EasingType` (`CubicBezier`)
//! - `net.minecraft.world.attribute.LerpFunction` + `util.ARGB.srgbLerp`
//! - `net.minecraft.world.level.MoonPhase`

/// Ticks in one overworld day (`Timeline.setPeriodTicks(24000)`).
pub const DAY_TICKS: i64 = 24_000;
/// `MoonPhase.COUNT` — eight phases.
pub const MOON_PHASE_COUNT: i64 = 8;
/// `MoonPhase.PHASE_LENGTH` — each phase lasts a full day.
pub const MOON_PHASE_LENGTH: i64 = 24_000;
/// The MOON timeline period: `24000 * MoonPhase.COUNT`.
pub const MOON_PERIOD_TICKS: i64 = MOON_PHASE_LENGTH * MOON_PHASE_COUNT;

// ---------------------------------------------------------------------------
// EasingType.CubicBezier — ported verbatim.
// ---------------------------------------------------------------------------

/// One axis of a cubic bezier as vanilla stores it: `curveFromControls`
/// precomputes the polynomial `((a·t + b)·t + c)·t` (the `t³/t²/t` coeffs of a
/// bezier with fixed endpoints 0 and 1).
#[derive(Clone, Copy, Debug)]
struct CubicCurve {
    a: f32,
    b: f32,
    c: f32,
}

impl CubicCurve {
    /// `EasingType.CubicBezier.curveFromControls`.
    fn from_controls(v1: f32, v2: f32) -> Self {
        Self {
            a: 3.0 * v1 - 3.0 * v2 + 1.0,
            b: -6.0 * v1 + 3.0 * v2,
            c: 3.0 * v1,
        }
    }

    fn sample(self, t: f32) -> f32 {
        ((self.a * t + self.b) * t + self.c) * t
    }

    fn sample_gradient(self, t: f32) -> f32 {
        (3.0 * self.a * t + 2.0 * self.b) * t + self.c
    }
}

/// `EasingType.CubicBezier`: solves `x(t) = x` by Newton-Raphson (4 iterations,
/// step clamped to ±0.25, tolerance 1e-5), falling back to bisection, then
/// evaluates `y(t)`. All constants match the decompile byte-for-byte.
#[derive(Clone, Copy, Debug)]
pub struct CubicBezier {
    x_curve: CubicCurve,
    y_curve: CubicCurve,
}

impl CubicBezier {
    const NEWTON_ITERATIONS: usize = 4;
    const MAX_STEP: f32 = 0.25;
    const TOLERANCE: f32 = 1.0e-5;

    /// `EasingType.cubicBezier(x1, y1, x2, y2)`.
    pub fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x_curve: CubicCurve::from_controls(x1, x2),
            y_curve: CubicCurve::from_controls(y1, y2),
        }
    }

    /// `EasingType.symmetricCubicBezier(x1, y1)` = `cubicBezier(x1, y1, 1-x1, 1-y1)`.
    pub fn symmetric(x1: f32, y1: f32) -> Self {
        Self::new(x1, y1, 1.0 - x1, 1.0 - y1)
    }

    /// `EasingType.apply` — the eased alpha.
    pub fn apply(self, x: f32) -> f32 {
        self.y_curve.sample(self.solve_t(x))
    }

    fn solve_t(self, x: f32) -> f32 {
        let mut t = x;
        for _ in 0..Self::NEWTON_ITERATIONS {
            let error = self.x_curve.sample(t) - x;
            if error.abs() < Self::TOLERANCE {
                return t;
            }
            let gradient = self.x_curve.sample_gradient(t);
            if gradient < Self::TOLERANCE {
                break;
            }
            t -= (error / gradient).clamp(-Self::MAX_STEP, Self::MAX_STEP);
        }
        self.solve_t_bisect(x, t)
    }

    fn solve_t_bisect(self, x: f32, initial_t: f32) -> f32 {
        let mut t0 = 0.0f32;
        let mut t1 = 1.0f32;
        let mut t = initial_t;
        // Mirrors the Java `for (t = initialT; t0 < t1; t = (t1 + t0) / 2.0F)`.
        while t0 < t1 {
            let error = self.x_curve.sample(t) - x;
            if error.abs() < Self::TOLERANCE {
                return t;
            }
            if error < 0.0 {
                t0 = t;
            } else {
                t1 = t;
            }
            t = (t1 + t0) / 2.0;
        }
        t
    }
}

/// `KeyframeTrack.easingType` — LINEAR by default; the angle tracks override to
/// the symmetric cubic bezier.
#[derive(Clone, Copy, Debug)]
pub enum Easing {
    Linear,
    Bezier(CubicBezier),
}

impl Easing {
    fn apply(self, alpha: f32) -> f32 {
        match self {
            Easing::Linear => alpha,
            Easing::Bezier(b) => b.apply(alpha),
        }
    }
}

// ---------------------------------------------------------------------------
// KeyframeTrackSampler — ported for a periodic track with ≥2 keyframes.
// ---------------------------------------------------------------------------

/// One baked segment (`KeyframeTrackSampler.Segment`).
#[derive(Clone, Copy)]
struct Segment<T> {
    from_value: T,
    from_ticks: i64,
    to_value: T,
    to_ticks: i64,
}

/// Bake the wrap-aware segment list exactly as `KeyframeTrackSampler
/// .bakeSegments` does for a track with a period and ≥2 keyframes: a leading
/// segment from the last keyframe (shifted back one period) into the first, the
/// interior keyframe pairs, then a trailing segment from the last into the
/// first (shifted forward one period).
fn bake_segments<T: Copy>(keyframes: &[(i64, T)], period: i64) -> Vec<Segment<T>> {
    debug_assert!(keyframes.len() >= 2, "celestial tracks have ≥2 keyframes");
    let first = keyframes[0];
    let last = keyframes[keyframes.len() - 1];
    let mut segments = Vec::with_capacity(keyframes.len() + 1);
    segments.push(Segment {
        from_value: last.1,
        from_ticks: last.0 - period,
        to_value: first.1,
        to_ticks: first.0,
    });
    for pair in keyframes.windows(2) {
        segments.push(Segment {
            from_value: pair[0].1,
            from_ticks: pair[0].0,
            to_value: pair[1].1,
            to_ticks: pair[1].0,
        });
    }
    segments.push(Segment {
        from_value: last.1,
        from_ticks: last.0,
        to_value: first.1,
        to_ticks: first.0 + period,
    });
    segments
}

/// `KeyframeTrackSampler.sample` for a periodic track: loop the tick into
/// `[0, period)`, find the segment, and lerp with the eased alpha. `lerp` is
/// the modifier's `argumentKeyframeLerp` (linear float, or `ARGB.srgbLerp`).
fn sample_track<T: Copy>(
    keyframes: &[(i64, T)],
    period: i64,
    easing: Easing,
    ticks: i64,
    lerp: impl Fn(f32, T, T) -> T,
) -> T {
    let segments = bake_segments(keyframes, period);
    let sample_ticks = ticks.rem_euclid(period);
    // getSegmentAt: first segment whose toTicks is strictly greater, else last.
    let segment = segments
        .iter()
        .find(|s| sample_ticks < s.to_ticks)
        .copied()
        .unwrap_or_else(|| *segments.last().unwrap());

    if sample_ticks <= segment.from_ticks {
        return segment.from_value;
    }
    if sample_ticks >= segment.to_ticks {
        return segment.to_value;
    }
    let alpha =
        (sample_ticks - segment.from_ticks) as f32 / (segment.to_ticks - segment.from_ticks) as f32;
    let eased = easing.apply(alpha);
    lerp(eased, segment.from_value, segment.to_value)
}

// ---------------------------------------------------------------------------
// Lerp functions (LerpFunction) — the exact ones the tracks resolve to.
// ---------------------------------------------------------------------------

/// `Mth.lerp` — the linear float lerp (`LerpFunction.ofFloat`), used by the
/// angle and star-brightness tracks.
fn lerp_float(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

/// `Mth.lerpInt` — `p0 + floor(alpha * (p1 - p0))`.
fn lerp_int(alpha: f32, p0: i32, p1: i32) -> i32 {
    p0 + (alpha * (p1 - p0) as f32).floor() as i32
}

/// `ARGB.srgbLerp` — per-channel integer lerp in gamma space, **including
/// alpha** (`LerpFunction.ofColor`). Despite the name it does *not* convert to
/// linear (`ARGB.linearLerp` is the one that does); it is a plain componentwise
/// `Mth.lerpInt`. This is what settles the "RGB vs ARGB" question: the
/// `SUNRISE_SUNSET_COLOR` track is an `ARGB_COLOR` override track, so all four
/// channels — alpha too — interpolate linearly in sRGB.
fn srgb_lerp(alpha: f32, p0: i32, p1: i32) -> i32 {
    let ch = |sh: u32| -> i32 {
        let a = (p0 >> sh) & 0xFF;
        let b = (p1 >> sh) & 0xFF;
        lerp_int(alpha, a, b) & 0xFF
    };
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

// ---------------------------------------------------------------------------
// The OVERWORLD_DAY tracks (verbatim keyframes).
// ---------------------------------------------------------------------------

/// `symmetricCubicBezier(0.362, 0.241)` — the sky-angle easing.
fn sky_angle_ease() -> Easing {
    Easing::Bezier(CubicBezier::symmetric(0.362, 0.241))
}

/// `SUN_ANGLE`: `setEasing(skyAngleEase).addKeyframe(6000, 360).addKeyframe(6000, 0)`.
const SUN_ANGLE_KF: &[(i64, f32)] = &[(6000, 360.0), (6000, 0.0)];
/// `MOON_ANGLE`: `.addKeyframe(6000, 540).addKeyframe(6000, 180)`.
const MOON_ANGLE_KF: &[(i64, f32)] = &[(6000, 540.0), (6000, 180.0)];
/// `STAR_ANGLE`: `.addKeyframe(6000, 360).addKeyframe(6000, 0)`.
const STAR_ANGLE_KF: &[(i64, f32)] = &[(6000, 360.0), (6000, 0.0)];

/// `STAR_BRIGHTNESS`: `FloatModifier.MAXIMUM`, default LINEAR ease. The base
/// value is 0.0 (`EnvironmentAttributes.STAR_BRIGHTNESS.defaultValue(0.0)`) and
/// every keyframe value is ≥ 0, so `max(0, track)` reduces to the track — we
/// sample it directly.
const STAR_BRIGHTNESS_KF: &[(i64, f32)] = &[
    (92, 0.037),
    (627, 0.0),
    (11373, 0.0),
    (11732, 0.016),
    (11959, 0.044),
    (12399, 0.143),
    (12729, 0.258),
    (13228, 0.5),
    (22772, 0.5),
    (23032, 0.364),
    (23356, 0.225),
    (23758, 0.101),
];

/// `SUNRISE_SUNSET_COLOR`: an `ARGB_COLOR` override track, default LINEAR ease,
/// `srgbLerp` interpolation. Values are the raw signed 32-bit ARGB ints from
/// the decompile.
const SUNRISE_SUNSET_KF: &[(i64, i32)] = &[
    (71, 1_609_540_403),
    (310, 703_969_843),
    (565, 117_167_155),
    (730, 16_770_355),
    (11270, 16_770_355),
    (11397, 83_679_283),
    (11522, 268_028_723),
    (11690, 703_969_843),
    (11929, 1_609_540_403),
    (12243, -1_310_226_637),
    (12358, -857_440_717),
    (12512, -371_166_669),
    (12613, -153_261_261),
    (12732, -19_242_189),
    (12841, -19_440_589),
    (13035, -321_760_973),
    (13252, -1_043_577_037),
    (13775, 918_435_635),
    (13888, 532_362_547),
    (14039, 163_001_139),
    (14192, 11_744_051),
    (21807, 11_678_515),
    (21961, 163_001_139),
    (22112, 532_362_547),
    (22225, 918_435_635),
    (22748, -1_043_577_037),
    (22965, -321_760_973),
    (23159, -19_440_589),
    (23272, -19_242_189),
    (23488, -371_166_669),
    (23642, -857_440_717),
    (23757, -1_310_226_637),
];

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// The clear-weather Overworld celestial state at one world-clock tick.
/// Angles are in **degrees** (matching `EnvironmentAttributes.*_ANGLE`, type
/// `ANGLE_DEGREES`); the renderer converts to radians and applies the exact
/// `SkyRenderer.renderSunMoonAndStars` transform chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Celestial {
    pub sun_angle_deg: f32,
    pub moon_angle_deg: f32,
    pub star_angle_deg: f32,
    /// 0..1; the sun/moon/star quads are hidden when this is 0.
    pub star_brightness: f32,
    /// Packed signed ARGB (alpha is meaningful: 0 means "no sunrise now").
    pub sunrise_sunset_color: i32,
    /// `MoonPhase.index()` 0..7 (0 = full moon), selecting the moon texture.
    pub moon_phase: u8,
}

impl Celestial {
    /// Sun angle in radians, as `SkyRenderer.extractRenderState` computes it.
    pub fn sun_angle_rad(&self) -> f32 {
        self.sun_angle_deg.to_radians()
    }
    pub fn moon_angle_rad(&self) -> f32 {
        self.moon_angle_deg.to_radians()
    }
    pub fn star_angle_rad(&self) -> f32 {
        self.star_angle_deg.to_radians()
    }
    /// Sunrise/sunset alpha 0..1 (`ARGB.alphaFloat`). `renderSunriseAndSunset`
    /// skips the fan when this is ≤ 0.001.
    pub fn sunrise_alpha(&self) -> f32 {
        ((self.sunrise_sunset_color >> 24) & 0xFF) as f32 / 255.0
    }
}

/// The moon phase index at a total world-clock tick. The MOON timeline has
/// period `24000 * 8` with one keyframe per phase at `phase.index() * 24000`
/// and a step (`ofStep(1.0)`) lerp, so within each 24000-tick window the phase
/// is constant — i.e. `floor(t / 24000) mod 8`, done with Euclidean arithmetic
/// so negative ticks wrap the same way `Math.floorMod` does.
pub fn moon_phase(total_ticks: i64) -> u8 {
    total_ticks
        .div_euclid(MOON_PHASE_LENGTH)
        .rem_euclid(MOON_PHASE_COUNT) as u8
}

/// Evaluate the clear-weather Overworld celestial state at a world-clock tick.
pub fn celestial_at(total_ticks: i64) -> Celestial {
    let ease = sky_angle_ease();
    Celestial {
        sun_angle_deg: sample_track(SUN_ANGLE_KF, DAY_TICKS, ease, total_ticks, lerp_float),
        moon_angle_deg: sample_track(MOON_ANGLE_KF, DAY_TICKS, ease, total_ticks, lerp_float),
        star_angle_deg: sample_track(STAR_ANGLE_KF, DAY_TICKS, ease, total_ticks, lerp_float),
        star_brightness: sample_track(
            STAR_BRIGHTNESS_KF,
            DAY_TICKS,
            Easing::Linear,
            total_ticks,
            lerp_float,
        ),
        sunrise_sunset_color: sample_track(
            SUNRISE_SUNSET_KF,
            DAY_TICKS,
            Easing::Linear,
            total_ticks,
            srgb_lerp,
        ),
        moon_phase: moon_phase(total_ticks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CubicBezier: fixed points and monotonicity ---

    #[test]
    fn bezier_fixed_points() {
        let b = CubicBezier::symmetric(0.362, 0.241);
        assert!(
            (b.apply(0.0) - 0.0).abs() < 1e-4,
            "apply(0) = {}",
            b.apply(0.0)
        );
        assert!(
            (b.apply(1.0) - 1.0).abs() < 1e-4,
            "apply(1) = {}",
            b.apply(1.0)
        );
        // Symmetric bezier is point-symmetric about (0.5, 0.5).
        let lo = b.apply(0.3);
        let hi = b.apply(0.7);
        assert!(
            ((lo + hi) - 1.0).abs() < 2e-3,
            "expected point symmetry: {lo} + {hi} ≈ 1"
        );
    }

    #[test]
    fn bezier_is_monotonic() {
        let b = CubicBezier::symmetric(0.362, 0.241);
        let mut prev = f32::MIN;
        for i in 0..=1000 {
            let y = b.apply(i as f32 / 1000.0);
            assert!(y + 1e-4 >= prev, "bezier must be non-decreasing at {i}");
            prev = y;
        }
    }

    // --- Sun/moon/star angle: the two-keyframe wrap semantics ---

    #[test]
    fn sun_angle_is_zero_at_noon() {
        // Tick 6000 = noon (`ClockTimeMarkers.NOON`). The sampler returns
        // segment C's fromValue (0) exactly at 6000 → sun straight up.
        assert_eq!(celestial_at(6000).sun_angle_deg, 0.0);
    }

    #[test]
    fn sun_angle_sweeps_full_circle_once_per_day() {
        // Just before noon the sun approaches 360; just after, it leaves 0.
        // Both map (mod 360) to the top, but the raw degrees straddle the seam.
        let before = celestial_at(5999).sun_angle_deg;
        let after = celestial_at(6001).sun_angle_deg;
        assert!(
            before > 350.0,
            "pre-noon sun angle {before} should near 360"
        );
        assert!(after < 10.0, "post-noon sun angle {after} should leave 0");
    }

    #[test]
    fn sun_angle_bezier_not_linear_at_quarter_day() {
        // At tick 0 (dawn, 6am) the sampler is on segment A: alpha = 0.75.
        // A *linear* track would give 0.75·360 = 270. The bezier bends it away
        // from the linear value — this is the whole reason to port the easing.
        let a = celestial_at(0).sun_angle_deg;
        assert!(
            (a - 270.0).abs() > 1.0,
            "bezier-eased dawn angle {a} must differ from the linear 270"
        );
        // But it stays in the correct half-turn quadrant (~sun at the horizon).
        assert!((200.0..340.0).contains(&a), "dawn angle {a} out of range");
    }

    #[test]
    fn moon_angle_leads_sun_by_180() {
        // MOON_ANGLE keyframes are the sun's + 180, so at any tick the moon
        // angle should equal the sun angle + 180 (mod 360).
        for &t in &[0i64, 1000, 6000, 12000, 18000, 23000] {
            let c = celestial_at(t);
            let diff = (c.moon_angle_deg - c.sun_angle_deg).rem_euclid(360.0);
            assert!(
                (diff - 180.0).abs() < 1e-2,
                "at {t}: moon {} vs sun {} not 180 apart",
                c.moon_angle_deg,
                c.sun_angle_deg
            );
        }
    }

    // --- Star brightness ---

    #[test]
    fn stars_dark_at_noon_bright_at_midnight() {
        assert_eq!(celestial_at(6000).star_brightness, 0.0, "no stars at noon");
        // Midnight plateau (13228..22772) is 0.5.
        assert_eq!(celestial_at(18000).star_brightness, 0.5);
    }

    #[test]
    fn star_brightness_interpolates_on_the_rising_edge() {
        // Between 12729 (0.258) and 13228 (0.5).
        let s = celestial_at((12729 + 13228) / 2).star_brightness;
        assert!(s > 0.258 && s < 0.5, "dusk star brightness {s} out of band");
    }

    // --- Sunrise/sunset colour (ARGB srgbLerp, alpha included) ---

    #[test]
    fn sunrise_color_exact_at_keyframe() {
        // At an exact keyframe tick the sampler returns that value verbatim
        // (sampleTicks <= fromTicks / >= toTicks short-circuits before lerp).
        assert_eq!(celestial_at(730).sunrise_sunset_color, 16_770_355);
        assert_eq!(celestial_at(11270).sunrise_sunset_color, 16_770_355);
    }

    #[test]
    fn sunrise_alpha_zero_at_midday_nonzero_at_dusk() {
        // 16770355 = 0x00FFECF3 → alpha byte 0 → no sunrise disc at midday.
        assert_eq!(celestial_at(6000).sunrise_alpha(), 0.0);
        // Deep dusk keyframe 12732 = -19242189 = 0xFEDA... → alpha ~0xFE.
        assert!(
            celestial_at(12732).sunrise_alpha() > 0.9,
            "expected a strong sunset alpha, got {}",
            celestial_at(12732).sunrise_alpha()
        );
    }

    #[test]
    fn sunrise_color_channel_lerp_is_gamma_space_argb() {
        // Between keyframe 71 (1609540403) and 310 (703969843), each of the
        // four channels — alpha included — must interpolate independently by
        // `Mth.lerpInt` (gamma space, floor). Deriving the expected value
        // channel-by-channel from the raw constants is the actual property:
        // it would reject a linear-space lerp, an RGB-only lerp (alpha frozen),
        // or a round-instead-of-floor lerp.
        let (k0, k1) = (1_609_540_403i32, 703_969_843i32);
        let mid = (71 + 310) / 2;
        let alpha = (mid - 71) as f32 / (310 - 71) as f32;
        let chan = |c: i32, sh: u32| (c >> sh) & 0xFF;
        let expect = |sh: u32| {
            let (p0, p1) = (chan(k0, sh), chan(k1, sh));
            p0 + ((alpha * (p1 - p0) as f32).floor() as i32)
        };
        let expected = (expect(24) << 24) | (expect(16) << 16) | (expect(8) << 8) | expect(0);
        assert_eq!(celestial_at(mid).sunrise_sunset_color, expected);
        // And alpha genuinely moved (not frozen), proving it's ARGB not RGB.
        assert_ne!(chan(k0, 24), chan(k1, 24));
    }

    // --- Moon phase ---

    #[test]
    fn moon_phase_cycles_eight_over_192000() {
        for i in 0..8i64 {
            // Sampled mid-window so it's unambiguous.
            assert_eq!(moon_phase(i * 24_000 + 12_000), i as u8);
        }
        // Wraps after 8 days.
        assert_eq!(moon_phase(192_000 + 6_000), moon_phase(6_000));
    }

    #[test]
    fn moon_phase_negative_ticks_wrap_like_floormod() {
        assert_eq!(moon_phase(-1), 7);
        assert_eq!(moon_phase(-24_000), 7);
        assert_eq!(moon_phase(-24_001), 6);
    }

    // --- Period wrapping ---

    #[test]
    fn celestial_state_wraps_by_day_and_moon_period() {
        // Angles/brightness/sunrise repeat every 24000; only the moon phase
        // differs across days.
        let a = celestial_at(6000);
        let b = celestial_at(6000 + DAY_TICKS);
        assert_eq!(a.sun_angle_deg, b.sun_angle_deg);
        assert_eq!(a.star_brightness, b.star_brightness);
        assert_eq!(a.sunrise_sunset_color, b.sunrise_sunset_color);
        // Full state (moon phase included) repeats every MOON_PERIOD_TICKS.
        assert_eq!(celestial_at(6000), celestial_at(6000 + MOON_PERIOD_TICKS));
    }
}
