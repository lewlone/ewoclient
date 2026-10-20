//! M282 — the HUD vignette's arithmetic (`Hud.updateVignetteBrightness` and
//! `Hud.extractVignette`).
//!
//! Every tick the camera's light is read at its eye, turned into the
//! dimension-curved brightness `Lightmap.getBrightness`, and the vignette's
//! own brightness follows `1 - that` by a hundredth of the difference — which
//! is why walking into a cave darkens the corners over several seconds. The
//! blit's colour is that brightness in grey, or, near a world border, a
//! red-shifted mix toward cyan-white as the warning strength rises. The pass
//! multiplies the frame by `1 - texture · colour` (`RenderPipelines.VIGNETTE`:
//! `BlendFunction(ZERO, ONE_MINUS_SRC_COLOR)`).

/// `Hud.vignetteBrightness`'s initial value.
pub const VIGNETTE_START: f32 = 1.0;

/// `LevelReader.getRawBrightness(pos, skyDarken)`: the block light or the
/// sky light less the sky's darkening, whichever is higher.
pub fn raw_brightness(block: u8, sky: u8, sky_darken: i32) -> i32 {
    (block as i32).max(sky as i32 - sky_darken)
}

/// `Lightmap.getBrightness(dimensionType, level)`:
/// `v = level / 15; lerp(ambientLight, v / (4 - 3v), 1)`.
pub fn level_brightness(ambient_light: f32, level: i32) -> f32 {
    let v = level.clamp(0, 15) as f32 / 15.0;
    let curved = v / (4.0 - 3.0 * v);
    curved + ambient_light * (1.0 - curved)
}

/// `Hud.updateVignetteBrightness`, one tick: toward
/// `clamp(1 - level_brightness, 0, 1)` by a hundredth of the gap.
pub fn step_vignette(current: f32, ambient_light: f32, raw_level: i32) -> f32 {
    let target = (1.0 - level_brightness(ambient_light, raw_level)).clamp(0.0, 1.0);
    current + (target - current) * 0.01
}

/// `Hud.extractVignette`'s border warning: `1 - distance / warningDistance`
/// inside the warning distance, where the warning distance is the larger of
/// `warningBlocks` and the moving border's `min(lerpSpeed · warningTime,
/// |lerpTarget - size|)`; 0 outside it.
pub fn border_warning(
    distance: f32,
    warning_blocks: i32,
    lerp_speed: f64,
    warning_time: i32,
    lerp_target: f64,
    size: f64,
) -> f32 {
    let moving = (lerp_speed * warning_time as f64).min((lerp_target - size).abs());
    let warning = (warning_blocks as f64).max(moving);
    if (distance as f64) < warning {
        (1.0 - distance as f64 / warning) as f32
    } else {
        0.0
    }
}

/// `Hud.extractVignette`'s blit colour (gamma rgb): the clamped brightness in
/// grey, or with a border warning `(b · (1 - w), b + (1 - b) · w, same)`.
pub fn vignette_color(brightness: f32, warning: f32) -> [f32; 3] {
    let b = brightness.clamp(0.0, 1.0);
    if warning > 0.0 {
        let w = warning.clamp(0.0, 1.0);
        let gb = b + (1.0 - b) * w;
        [b * (1.0 - w), gb, gb]
    } else {
        [b, b, b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full light gives no vignette and darkness a full one (in a dimension
    /// with no ambient light); the follow is a hundredth a tick; the border
    /// mix is vanilla's arithmetic at its two ends and the middle.
    #[test]
    fn the_vignette_follows_the_light_and_reddens_at_the_border() {
        assert_eq!(level_brightness(0.0, 15), 1.0);
        assert_eq!(level_brightness(0.0, 0), 0.0);
        assert!((level_brightness(0.1, 0) - 0.1).abs() < 1e-6);
        assert!((level_brightness(0.0, 8) - (8.0 / 15.0) / (4.0 - 3.0 * 8.0 / 15.0)).abs() < 1e-6);
        assert_eq!(raw_brightness(3, 15, 11), 4);
        assert_eq!(raw_brightness(7, 15, 11), 7);
        let v = step_vignette(VIGNETTE_START, 0.0, 15);
        assert!((v - 0.99).abs() < 1e-6, "{v}");
        let mut v = 0.0;
        for _ in 0..1000 {
            v = step_vignette(v, 0.0, 0);
        }
        assert!(v > 0.99, "{v}");
        assert_eq!(vignette_color(0.4, 0.0), [0.4, 0.4, 0.4]);
        assert_eq!(vignette_color(0.4, 1.0), [0.0, 1.0, 1.0]);
        let [r, g, _] = vignette_color(0.4, 0.5);
        assert!((r - 0.2).abs() < 1e-6 && (g - 0.7).abs() < 1e-6);
        assert_eq!(border_warning(10.0, 5, 0.0, 15, 100.0, 100.0), 0.0);
        assert!((border_warning(2.5, 5, 0.0, 15, 100.0, 100.0) - 0.5).abs() < 1e-6);
        // A moving border widens the warning: speed 1 a tick for 15 ticks.
        assert!((border_warning(7.5, 5, 1.0, 15, 50.0, 100.0) - 0.5).abs() < 1e-6);
    }
}
