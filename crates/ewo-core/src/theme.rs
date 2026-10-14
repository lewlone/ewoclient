//! Theme tokens. The Velvet theme is the default (and only) v1 theme.
//!
//! The first block of fields mirrors the CSS `:root` declaration in
//! `StyleSheet1` byte-for-byte and keeps its variable names (`--bg-wine-a` is
//! `bg_wine_a`, ...). The second block holds the *inline* hues the prototype
//! uses outside `:root` (the warm-white `#FFF6F0`, the error text `#D4889A`,
//! the modal card fills, ...): every colour the renderer paints comes from one
//! of these fields, none from a literal in a draw function. See `CLAUDE.md`
//! and `.claude/rules/ewo-visual.md` for the canonical list.
//!
//! [`Settings`] holds the five user-tunable parameters (`motion_speed`,
//! `breath_amp`, `density`, `warmth`, `accent_hue_shift`). Of these,
//! `accent_hue_shift` acts on the theme itself: [`Theme::resolved`] rotates
//! the accent tokens' OkLCH hue, and the renderer paints with the resolved
//! theme.

use crate::color::{Srgb, Srgba};
use crate::easing::CubicBezier;

#[derive(Copy, Clone, Debug)]
pub struct Theme {
    // ── CSS `:root` ────────────────────────────────────────────────────
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
    /// Error colour, "rose-red within the same family". Semantic, so
    /// `accent_hue_shift` leaves it (and the other `ember` hues) alone.
    pub accent_ember: Srgb,

    // Motion
    pub silk: CubicBezier,

    // ── Inline hues from the prototype's rules (not `:root` variables) ─
    // Text neutrals
    /// `#FFF6F0` warm-white: hot text, caret, pressed-button label.
    pub text_pearl_hot: Srgb,
    /// `#FFF0F4` pearl highlight: the hover end of the menu-label ramp.
    pub text_pearl_highlight: Srgb,
    /// `#FAF0F2` the bright core of a pearl-dust mote.
    pub pearl_dust_core: Srgb,
    /// `#FFE6EE` pale bloom: the slider thumb / progress-bar head highlight.
    pub pearl_bloom: Srgb,
    /// `#FFE1E6` the press-ripple core.
    pub pearl_ripple: Srgb,
    /// `#F2E8ED` the cursor-speck core.
    pub pearl_speck: Srgb,
    /// `#C4AFB5` mid-pearl: secondary body text.
    pub text_mid_pearl: Srgb,
    /// `#A38F95` the main-menu tagline grey.
    pub text_tagline: Srgb,
    /// `#6B555C` deep mauve: hints, placeholders, disabled text.
    pub text_mauve_deep: Srgb,

    // Accent variants
    /// `#D4A8B8` the dusky start of the progress-bar / slider fill gradient.
    pub accent_rose_dusk: Srgb,
    /// `#F4D4DE` the pale highlight stop of the progress-bar gradient.
    pub accent_rose_pale: Srgb,
    /// `#E5B0BD` danger-ghost-button hover label.
    pub accent_rose_soft: Srgb,
    /// `#C8A4B4` the toggle knob's shaded edge.
    pub accent_rose_edge: Srgb,
    /// `#B4648C` the app window's inner berry glow (`rgba(180, 100, 140)`).
    pub accent_berry_glow: Srgb,
    /// `#D4889A` error text and the light end of the error gradient.
    pub accent_ember_text: Srgb,
    /// `#A35A6C` the deep start of the error gradient.
    pub accent_ember_deep: Srgb,
    /// `#F4C8D0` the window-close icon on hover.
    pub accent_ember_icon: Srgb,

    // Dark chrome fills (modals, glass)
    /// `#080206` the modal scrim.
    pub bg_scrim: Srgb,
    /// `#26121E` the top of a modal card / dropdown menu body.
    pub bg_card_top: Srgb,
    /// `#1C0C16` the bottom of a modal card / dropdown menu body.
    pub bg_card_bottom: Srgb,
    /// `#1C0A14` the dark-mauve fill under a glass panel's tint.
    pub bg_glass_fill: Srgb,
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

        text_pearl_hot: Srgb::from_hex(0xFFF6F0),
        text_pearl_highlight: Srgb::from_hex(0xFFF0F4),
        pearl_dust_core: Srgb::from_hex(0xFAF0F2),
        pearl_bloom: Srgb::from_hex(0xFFE6EE),
        pearl_ripple: Srgb::from_hex(0xFFE1E6),
        pearl_speck: Srgb::from_hex(0xF2E8ED),
        text_mid_pearl: Srgb::from_hex(0xC4AFB5),
        text_tagline: Srgb::from_hex(0xA38F95),
        text_mauve_deep: Srgb::from_hex(0x6B555C),

        accent_rose_dusk: Srgb::from_hex(0xD4A8B8),
        accent_rose_pale: Srgb::from_hex(0xF4D4DE),
        accent_rose_soft: Srgb::from_hex(0xE5B0BD),
        accent_rose_edge: Srgb::from_hex(0xC8A4B4),
        accent_berry_glow: Srgb::from_hex(0xB4648C),
        accent_ember_text: Srgb::from_hex(0xD4889A),
        accent_ember_deep: Srgb::from_hex(0xA35A6C),
        accent_ember_icon: Srgb::from_hex(0xF4C8D0),

        bg_scrim: Srgb::from_hex(0x080206),
        bg_card_top: Srgb::from_hex(0x26121E),
        bg_card_bottom: Srgb::from_hex(0x1C0C16),
        bg_glass_fill: Srgb::from_hex(0x1C0A14),
    };

    /// This theme with the accent hues rotated by `degrees` (OkLCH, constant
    /// lightness and chroma). The shifted set is the berry / rose / lavender /
    /// champagne accents and their variants; neutrals, backgrounds and the
    /// ember (error) family keep their hue. `0°` returns the theme unchanged.
    pub fn with_accent_hue_shift(&self, degrees: f32) -> Self {
        if degrees == 0.0 {
            return *self;
        }
        let s = |c: Srgb| c.hue_shifted(degrees);
        Self {
            accent_berry: s(self.accent_berry),
            accent_rose: s(self.accent_rose),
            accent_lav: s(self.accent_lav),
            accent_champ: s(self.accent_champ),
            accent_rose_dusk: s(self.accent_rose_dusk),
            accent_rose_pale: s(self.accent_rose_pale),
            accent_rose_soft: s(self.accent_rose_soft),
            accent_rose_edge: s(self.accent_rose_edge),
            accent_berry_glow: s(self.accent_berry_glow),
            ..*self
        }
    }

    /// The theme the renderer paints with: this one with the user's
    /// `accent_hue_shift` applied.
    pub fn resolved(&self, settings: &Settings) -> Self {
        self.with_accent_hue_shift(settings.accent_hue_shift)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Settings {
    /// Global animation rate (CSS `--motion-speed`): every ambient period is
    /// `base / motion_speed`. Apply it through [`crate::motion::scaled_time`].
    pub motion_speed: f32,
    pub breath_amp: f32,
    pub density: f32,
    pub warmth: f32,
    /// Degrees; see [`Theme::with_accent_hue_shift`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::OkLch;

    #[test]
    fn default_settings_do_not_touch_the_theme() {
        let t = Theme::VELVET.resolved(&Settings::default());
        assert_eq!(t.accent_rose, Theme::VELVET.accent_rose);
        assert_eq!(t.accent_berry, Theme::VELVET.accent_berry);
        assert_eq!(t.accent_rose_dusk, Theme::VELVET.accent_rose_dusk);
    }

    #[test]
    fn hue_shift_moves_accents_and_only_accents() {
        let shifted = Theme::VELVET.with_accent_hue_shift(120.0);
        assert!(shifted.accent_rose != Theme::VELVET.accent_rose);
        assert!(shifted.accent_berry != Theme::VELVET.accent_berry);
        assert!(shifted.accent_lav != Theme::VELVET.accent_lav);
        assert!(shifted.accent_champ != Theme::VELVET.accent_champ);
        assert_eq!(shifted.text_pearl, Theme::VELVET.text_pearl);
        assert_eq!(shifted.text_mauve, Theme::VELVET.text_mauve);
        assert_eq!(shifted.accent_ember, Theme::VELVET.accent_ember);
        assert_eq!(shifted.bg_wine_a, Theme::VELVET.bg_wine_a);
        let dh = (OkLch::from_srgb(shifted.accent_rose).h
            - OkLch::from_srgb(Theme::VELVET.accent_rose).h)
            .rem_euclid(360.0);
        assert!((dh - 120.0).abs() < 8.0, "rose hue moved {dh}");
    }

    #[test]
    fn settings_drive_the_resolved_theme() {
        let settings = Settings {
            accent_hue_shift: -60.0,
            ..Settings::default()
        };
        assert_eq!(
            Theme::VELVET.resolved(&settings).accent_rose,
            Theme::VELVET.with_accent_hue_shift(-60.0).accent_rose
        );
    }
}
