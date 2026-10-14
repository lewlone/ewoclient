//! Theme tokens as Skia colours: the one place a draw function gets a colour
//! from.
//!
//! The renderer's draw functions are free functions without a shared context
//! (and there are hundreds of them), so the resolved [`Theme`] is published
//! per thread instead of being threaded through every signature:
//! [`draw_frame`](crate::app_window::draw_frame) and
//! [`Backdrop::draw`](crate::backdrop::Backdrop::draw) call [`set_active`]
//! with the frame's theme and settings, and every `tokens::xxx()` below reads
//! it. The resolved theme already has the user's `accent_hue_shift` applied
//! (see [`Theme::resolved`]), so changing the slider repaints every accent.
//! Nothing here caches across frames: a slider drag is a one-frame event.
//!
//! Each token comes in two forms:
//!
//! * `tokens::accent_rose()` is an opaque [`Color`],
//! * `tokens::accent_rose_a(0.08)` is a [`Color4f`] at that alpha (straight,
//!   gamma-encoded floats, i.e. `229.0 / 255.0` etc. exactly as the draw
//!   functions used to spell them).
//!
//! The default thread state is `Theme::VELVET` with no shift, so code that
//! never calls [`set_active`] (examples, the HUD's reuse of widgets, tests)
//! paints the stock Velvet palette.

use std::cell::Cell;

use ewo_core::{CubicBezier, OkLch, Settings, Srgb, Theme};
use skia_safe::{Color, Color4f};

#[derive(Copy, Clone)]
struct Active {
    theme: Theme,
    hue_shift: f32,
}

thread_local! {
    static ACTIVE: Cell<Active> = const {
        Cell::new(Active {
            theme: Theme::VELVET,
            hue_shift: 0.0,
        })
    };
}

/// Publish `theme` (with `settings.accent_hue_shift` applied) as the palette
/// for every `tokens::*` call on this thread.
pub fn set_active(theme: &Theme, settings: &Settings) {
    ACTIVE.with(|a| {
        a.set(Active {
            theme: theme.resolved(settings),
            hue_shift: settings.accent_hue_shift,
        })
    });
}

/// The resolved theme currently in force on this thread.
pub fn active() -> Theme {
    ACTIVE.with(|a| a.get().theme)
}

/// The accent hue shift in force, in degrees.
pub fn hue_shift() -> f32 {
    ACTIVE.with(|a| a.get().hue_shift)
}

/// An OkLCH accent colour with the active hue shift applied, for the layers
/// the prototype writes in `oklch(...)` (caustics, velvet folds, petals).
/// Exact identity at a shift of 0.
pub fn shift_oklch(c: OkLch) -> OkLch {
    c.rotate_hue(hue_shift())
}

/// `--silk`, the signature easing curve (`cubic-bezier(0.22, 1, 0.36, 1)`).
pub fn silk() -> CubicBezier {
    active().silk
}

/// An opaque Skia colour for a theme token value.
pub fn rgb(c: Srgb) -> Color {
    Color::from_argb(0xFF, c.r, c.g, c.b)
}

/// A straight-alpha float colour for a theme token value.
pub fn rgba(c: Srgb, alpha: f32) -> Color4f {
    Color4f::new(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        alpha,
    )
}

/// An accent-family colour the prototype wrote as a hand-rounded float triple
/// (`rgba(0.90, 0.72, 0.77, a)`, a rounding of `accent_rose`) instead of its
/// hex token. At the default theme it is returned **exactly as written**, so
/// the pixels are the ones the screen always had; under an accent hue shift it
/// rotates with the accents. Prefer the named tokens; this exists for the
/// literals whose float value no 8-bit token reproduces.
pub fn tint(r: f32, g: f32, b: f32, alpha: f32) -> Color4f {
    let shift = hue_shift();
    if shift == 0.0 {
        return Color4f::new(r, g, b, alpha);
    }
    let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    rgba(
        Srgb {
            r: q(r),
            g: q(g),
            b: q(b),
        }
        .hue_shifted(shift),
        alpha,
    )
}

/// Linear blend of two token values in gamma-encoded 8-bit space, as float
/// channels. `t = 0` is `a`, `t = 1` is `b`.
pub fn mix(a: Srgb, b: Srgb, t: f32) -> Color4f {
    let ch = |a: u8, b: u8| (a as f32 + (b as i32 - a as i32) as f32 * t) / 255.0;
    Color4f::new(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b), 1.0)
}

/// `--panel-glass` (`rgba(180, 130, 160, 0.06)`) at a caller-chosen alpha: the
/// token's colour, not its resting 0.06 alpha.
pub fn panel_glass_a(alpha: f32) -> Color4f {
    let g = active().panel_glass;
    rgba(
        Srgb {
            r: g.r,
            g: g.g,
            b: g.b,
        },
        alpha,
    )
}

macro_rules! tokens {
    ($($field:ident => $opaque:ident, $alpha:ident;)*) => {$(
        #[doc = concat!("`Theme::", stringify!($field), "` as an opaque colour.")]
        pub fn $opaque() -> Color {
            rgb(active().$field)
        }

        #[doc = concat!("`Theme::", stringify!($field), "` at `alpha` (0..1).")]
        pub fn $alpha(alpha: f32) -> Color4f {
            rgba(active().$field, alpha)
        }
    )*};
}

tokens! {
    bg_core => bg_core, bg_core_a;
    bg_wine_a => bg_wine_a, bg_wine_a_a;
    bg_wine_b => bg_wine_b, bg_wine_b_a;
    text_pearl => text_pearl, text_pearl_a;
    text_mauve => text_mauve, text_mauve_a;
    accent_berry => accent_berry, accent_berry_a;
    accent_rose => accent_rose, accent_rose_a;
    accent_lav => accent_lav, accent_lav_a;
    accent_champ => accent_champ, accent_champ_a;
    accent_ember => accent_ember, accent_ember_a;
    text_pearl_hot => text_pearl_hot, text_pearl_hot_a;
    text_pearl_highlight => text_pearl_highlight, text_pearl_highlight_a;
    pearl_dust_core => pearl_dust_core, pearl_dust_core_a;
    pearl_bloom => pearl_bloom, pearl_bloom_a;
    pearl_ripple => pearl_ripple, pearl_ripple_a;
    pearl_speck => pearl_speck, pearl_speck_a;
    text_mid_pearl => text_mid_pearl, text_mid_pearl_a;
    text_tagline => text_tagline, text_tagline_a;
    text_mauve_deep => text_mauve_deep, text_mauve_deep_a;
    accent_rose_dusk => accent_rose_dusk, accent_rose_dusk_a;
    accent_rose_pale => accent_rose_pale, accent_rose_pale_a;
    accent_rose_soft => accent_rose_soft, accent_rose_soft_a;
    accent_rose_edge => accent_rose_edge, accent_rose_edge_a;
    accent_berry_glow => accent_berry_glow, accent_berry_glow_a;
    accent_ember_text => accent_ember_text, accent_ember_text_a;
    accent_ember_deep => accent_ember_deep, accent_ember_deep_a;
    accent_ember_icon => accent_ember_icon, accent_ember_icon_a;
    bg_scrim => bg_scrim, bg_scrim_a;
    bg_card_top => bg_card_top, bg_card_top_a;
    bg_card_bottom => bg_card_bottom, bg_card_bottom_a;
    bg_glass_fill => bg_glass_fill, bg_glass_fill_a;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_stock_velvet() {
        // Fresh test thread: nothing has called `set_active`.
        assert_eq!(hue_shift(), 0.0);
        let c = accent_rose_a(0.5);
        assert_eq!(
            (c.r, c.g, c.b, c.a),
            (229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.5)
        );
        assert_eq!(text_pearl(), Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
    }

    #[test]
    fn set_active_applies_the_hue_shift_to_accents_only() {
        let settings = Settings {
            accent_hue_shift: 90.0,
            ..Settings::default()
        };
        set_active(&Theme::VELVET, &settings);
        assert_ne!(accent_rose(), rgb(Theme::VELVET.accent_rose));
        assert_eq!(text_pearl(), rgb(Theme::VELVET.text_pearl));
        assert_eq!(accent_ember(), rgb(Theme::VELVET.accent_ember));
        let k = OkLch::new(0.85, 0.06, 340.0, 0.12);
        assert_eq!(shift_oklch(k).h, 70.0);
        set_active(&Theme::VELVET, &Settings::default());
        assert_eq!(accent_rose(), rgb(Theme::VELVET.accent_rose));
        assert_eq!(shift_oklch(k), k);
    }

    #[test]
    fn tint_is_verbatim_until_the_accents_move() {
        let c = tint(0.90, 0.72, 0.77, 0.3);
        assert_eq!((c.r, c.g, c.b, c.a), (0.90, 0.72, 0.77, 0.3));
        set_active(
            &Theme::VELVET,
            &Settings {
                accent_hue_shift: 120.0,
                ..Settings::default()
            },
        );
        let moved = tint(0.90, 0.72, 0.77, 0.3);
        assert_ne!((moved.r, moved.g, moved.b), (0.90, 0.72, 0.77));
        assert_eq!(moved.a, 0.3);
        set_active(&Theme::VELVET, &Settings::default());
    }

    #[test]
    fn mix_is_the_menu_label_ramp() {
        let a = Theme::VELVET.text_pearl;
        let b = Theme::VELVET.text_pearl_highlight;
        let end = mix(a, b, 1.0);
        assert_eq!((end.r, end.g, end.b), (1.0, 240.0 / 255.0, 244.0 / 255.0));
        let start = mix(a, b, 0.0);
        assert_eq!(start.r, 244.0 / 255.0);
    }
}
