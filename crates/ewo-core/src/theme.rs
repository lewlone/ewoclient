//! Theme tokens. The Velvet theme is the default (and only) v1 theme.
//!
//! Values must match the CSS `:root` declaration in `StyleSheet1` byte-for-byte.
//! See `CLAUDE.md` for the canonical token list and the four user-tunable
//! parameters (`motion_speed`, `breath_amp`, `density`, `warmth`, `accent_hue_shift`).

use crate::color::{Srgb, Srgba};
use crate::easing::CubicBezier;

#[derive(Copy, Clone, Debug)]
pub struct Theme {
    // Backgrounds
    pub bg_core: Srgb,
    pub bg_wine_a: Srgb,
    pub bg_wine_b: Srgb,
    pub panel_glass: Srgba,

    // Text
    pub text_pearl: Srgb,
    pub text_mauve: Srgb,

    // Accents
    pub accent_berry: Srgb,
    pub accent_rose: Srgb,
    pub accent_lav: Srgb,
    pub accent_champ: Srgb,
    pub accent_ember: Srgb,

    // Motion
    pub silk: CubicBezier,
}

impl Theme {
    pub const VELVET: Self = Self {
        bg_core: Srgb::from_hex(0x000000),
        bg_wine_a: Srgb::from_hex(0x0A0006),
        bg_wine_b: Srgb::from_hex(0x120010),
        panel_glass: Srgba::from_hex_a(0xB482A0, 0.06),

        text_pearl: Srgb::from_hex(0xF4E8EA),
        text_mauve: Srgb::from_hex(0x9A8087),

        accent_berry: Srgb::from_hex(0xB47491),
        accent_rose: Srgb::from_hex(0xE5B8C5),
        accent_lav: Srgb::from_hex(0xC9A5D4),
        accent_champ: Srgb::from_hex(0xE8D4A8),
        accent_ember: Srgb::from_hex(0xC96A7A),

        silk: CubicBezier::SILK,
    };
}

#[derive(Copy, Clone, Debug)]
pub struct Settings {
    pub motion_speed: f32,
    pub breath_amp: f32,
    pub density: f32,
    pub warmth: f32,
    pub accent_hue_shift: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            motion_speed: 1.0,
            breath_amp: 1.0,
            density: 1.0,
            warmth: 0.6,
            accent_hue_shift: 0.0,
        }
    }
}
