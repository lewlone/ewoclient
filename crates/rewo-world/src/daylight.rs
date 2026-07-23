//! The overworld day/night cycle.
//!
//! 26.x replaced the old hard-coded `getSkyDarken` with a **keyframed
//! timeline** driving environment attributes. `Timelines.OVERWORLD_DAY` is a
//! 24000-tick period (`setPeriodTicks(24000)`) carrying one track per
//! attribute; the tracks here are transcribed from it verbatim.
//!
//! Note what is and is not affected. The chunk's stored sky light does **not**
//! change with the time of day — it stays 15 under open sky. What changes is
//! `SKY_LIGHT_FACTOR`, the multiplier the lightmap shader applies to the sky
//! half of the light. That is why the two channels are kept separate all the
//! way to the fragment shader: a sunset is a uniform update, not a remesh, and
//! block light (a torch) is unaffected by it.
//!
//! `KeyframeTrack`'s default easing is `EasingType.LINEAR` and none of these
//! tracks override it, so interpolation is a straight lerp. The period wraps:
//! the segment after the last keyframe runs to the first one plus 24000.

/// Ticks in one overworld day (`Timeline.setPeriodTicks`).
pub const DAY_TICKS: i64 = 24000;

/// `Timelines.NIGHT_SKY_LIGHT_COLOR` — sky light goes blue at night.
const NIGHT_SKY_LIGHT_COLOR: [f32; 3] = [0.48, 0.48, 1.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
/// `NIGHT_FOG_COLOR_MULTIPLIER_{START,END}`.
const NIGHT_FOG_START: [f32; 3] = [0.05, 0.05, 0.09];
const NIGHT_FOG_END: [f32; 3] = [0.09, 0.09, 0.09];

/// `SKY_LIGHT_FACTOR` multiplier track (base 1.0).
const SKY_FACTOR: &[(i64, f32)] = &[(730, 1.0), (11270, 1.0), (13140, 0.24), (22860, 0.24)];
/// `SKY_LIGHT_COLOR` multiplier track (base white).
const SKY_LIGHT_COLOR: &[(i64, [f32; 3])] = &[
    (730, WHITE),
    (11270, WHITE),
    (13140, NIGHT_SKY_LIGHT_COLOR),
    (22860, NIGHT_SKY_LIGHT_COLOR),
];
/// `SKY_COLOR` multiplier track — the rendered sky fades to black at night.
const SKY_COLOR: &[(i64, [f32; 3])] = &[
    (133, WHITE),
    (11867, WHITE),
    (13670, BLACK),
    (22330, BLACK),
];
/// `FOG_COLOR` multiplier track.
const FOG_COLOR: &[(i64, [f32; 3])] = &[
    (133, WHITE),
    (11867, WHITE),
    (13670, NIGHT_FOG_START),
    (22330, NIGHT_FOG_END),
];

/// Everything the renderer needs for one instant of the cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyLighting {
    /// Scales the sky half of the lightmap. 1.0 at midday, 0.24 at midnight.
    pub light_factor: f32,
    /// Tints the sky half — white by day, blue at night.
    pub light_color: [f32; 3],
    /// Multiplies the rendered sky gradient (→ black at night).
    pub sky_color: [f32; 3],
    /// Multiplies the distance-fog colour.
    pub fog_color: [f32; 3],
}

impl SkyLighting {
    /// Full daylight — the default before any `set_time` arrives, and what
    /// dimensions with a fixed time use.
    pub const DAY: Self = Self {
        light_factor: 1.0,
        light_color: WHITE,
        sky_color: WHITE,
        fog_color: WHITE,
    };
}

/// Sample a linear keyframe track at `t` (0..`DAY_TICKS`), wrapping.
fn sample<T: Copy>(track: &[(i64, T)], t: i64, lerp: impl Fn(T, T, f32) -> T) -> T {
    debug_assert!(!track.is_empty());
    let n = track.len();
    // Before the first keyframe the cycle is still on the wrap segment from
    // the last one, so start there and walk forward.
    for i in 0..n {
        let (t0, v0) = track[i];
        let (mut t1, v1) = track[(i + 1) % n];
        if t1 <= t0 {
            t1 += DAY_TICKS; // the wrap segment
        }
        let t = if t < t0 { t + DAY_TICKS } else { t };
        if t >= t0 && t < t1 {
            let f = (t - t0) as f32 / (t1 - t0) as f32;
            return lerp(v0, v1, f);
        }
    }
    track[0].1
}

fn lerp_f32(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f
}

fn lerp_rgb(a: [f32; 3], b: [f32; 3], f: f32) -> [f32; 3] {
    [
        lerp_f32(a[0], b[0], f),
        lerp_f32(a[1], b[1], f),
        lerp_f32(a[2], b[2], f),
    ]
}

/// Evaluate the overworld cycle at a world-clock tick.
pub fn sky_lighting(total_ticks: i64) -> SkyLighting {
    let t = total_ticks.rem_euclid(DAY_TICKS);
    SkyLighting {
        light_factor: sample(SKY_FACTOR, t, lerp_f32),
        light_color: sample(SKY_LIGHT_COLOR, t, lerp_rgb),
        sky_color: sample(SKY_COLOR, t, lerp_rgb),
        fog_color: sample(FOG_COLOR, t, lerp_rgb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midday_is_full_daylight() {
        // Noon is tick 6000 (`ClockTimeMarkers.NOON`), inside the flat
        // 730..11270 daytime plateau.
        let s = sky_lighting(6000);
        assert_eq!(s.light_factor, 1.0);
        assert_eq!(s.light_color, WHITE);
        assert_eq!(s.sky_color, WHITE);
    }

    #[test]
    fn midnight_is_the_night_values() {
        // Midnight is tick 18000, inside the flat 13140..22860 night plateau.
        let s = sky_lighting(18000);
        assert_eq!(s.light_factor, 0.24);
        assert_eq!(s.light_color, NIGHT_SKY_LIGHT_COLOR);
        assert_eq!(s.sky_color, BLACK);
    }

    #[test]
    fn dusk_interpolates_between_the_plateaus() {
        // Halfway through the 11270..13140 falling edge.
        let s = sky_lighting((11270 + 13140) / 2);
        assert!(
            s.light_factor > 0.24 && s.light_factor < 1.0,
            "mid-dusk factor {} should be between the plateaus",
            s.light_factor
        );
    }

    #[test]
    fn the_cycle_wraps_through_dawn() {
        // 22860 → 730 (+24000) is the wrap segment: night climbing back to
        // day. A naive scan that never wraps would return the first keyframe.
        let pre_dawn = sky_lighting(23500);
        assert!(
            pre_dawn.light_factor > 0.24 && pre_dawn.light_factor < 1.0,
            "pre-dawn factor {} should be rising",
            pre_dawn.light_factor
        );
        // And it must be monotonically brighter closer to the day plateau.
        assert!(sky_lighting(24000 + 500).light_factor > pre_dawn.light_factor);
    }

    #[test]
    fn negative_and_multi_day_ticks_normalise() {
        assert_eq!(sky_lighting(6000), sky_lighting(6000 + DAY_TICKS * 7));
        assert_eq!(sky_lighting(6000), sky_lighting(6000 - DAY_TICKS * 3));
    }
}
