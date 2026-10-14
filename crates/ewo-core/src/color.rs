//! Color types. sRGB linear, sRGB encoded, oklch.
//!
//! The prototype CSS uses oklch for several gradients (caustics, velvet folds,
//! petals) "for consistent luminance". We need oklch → linear-sRGB conversion
//! so gradient interpolation matches what the browser does.
//!
//! Conversion follows CSS Color Module Level 4 §11.3 (OKLab matrix). The
//! reverse direction (sRGB → oklch) exists so the theme's
//! `--accent-hue-shift` can rotate a token's hue without touching its
//! lightness or chroma ([`Srgb::hue_shifted`]).

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Srgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Srgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct OkLch {
    /// Lightness (0..1)
    pub l: f32,
    /// Chroma (0..~0.4 for typical sRGB-displayable colors)
    pub c: f32,
    /// Hue in degrees (0..360)
    pub h: f32,
    /// Alpha (0..1) — straight, not premultiplied.
    pub a: f32,
}

impl Srgb {
    pub const fn from_hex(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xFF) as u8,
            g: ((rgb >> 8) & 0xFF) as u8,
            b: (rgb & 0xFF) as u8,
        }
    }

    /// Straight-alpha [0..1] tuple, gamma-encoded sRGB. Useful for handing
    /// off to renderers that accept gamma-space float colors.
    pub fn to_f32(&self) -> [f32; 3] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        ]
    }
}

impl Srgb {
    /// This colour with its OkLCH hue rotated by `degrees`; lightness and
    /// chroma are kept (chroma is reduced only when the rotated colour would
    /// leave the sRGB gamut). `0°` returns the colour untouched, so a theme
    /// with no shift is bit-identical to the unshifted token.
    pub fn hue_shifted(self, degrees: f32) -> Self {
        if degrees == 0.0 {
            return self;
        }
        OkLch::from_srgb(self).rotate_hue(degrees).to_srgb()
    }
}

impl Srgba {
    /// See [`Srgb::hue_shifted`]; alpha is untouched.
    pub fn hue_shifted(self, degrees: f32) -> Self {
        let c = Srgb {
            r: self.r,
            g: self.g,
            b: self.b,
        }
        .hue_shifted(degrees);
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a: self.a,
        }
    }

    pub const fn from_hex_a(rgb: u32, a: f32) -> Self {
        let c = Srgb::from_hex(rgb);
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a,
        }
    }

    pub fn to_f32(&self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a,
        ]
    }
}

impl OkLch {
    pub const fn new(l: f32, c: f32, h: f32, a: f32) -> Self {
        Self { l, c, h, a }
    }

    /// The OkLCH form of an sRGB colour (opaque).
    pub fn from_srgb(c: Srgb) -> Self {
        let r = srgb_u8_to_linear(c.r);
        let g = srgb_u8_to_linear(c.g);
        let b = srgb_u8_to_linear(c.b);

        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();

        let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
        let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;

        let chroma = a.hypot(b);
        // Greys have no hue; report 0 rather than atan2's noise.
        let hue = if chroma < 1e-6 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        };
        Self {
            l: lightness,
            c: chroma,
            h: hue,
            a: 1.0,
        }
    }

    /// The same colour with its hue rotated by `degrees`. `0°` is an exact
    /// identity (no wrap, no float round trip).
    pub fn rotate_hue(self, degrees: f32) -> Self {
        if degrees == 0.0 {
            return self;
        }
        Self {
            h: (self.h + degrees).rem_euclid(360.0),
            ..self
        }
    }

    /// Gamma-encoded 8-bit sRGB (alpha dropped). Out-of-gamut colours are
    /// brought in by reducing chroma at constant lightness and hue, not by
    /// clamping channels, so a rotated hue keeps its character.
    pub fn to_srgb(&self) -> Srgb {
        let mut c = *self;
        if !c.in_gamut() {
            let (mut lo, mut hi) = (0.0_f32, c.c);
            for _ in 0..16 {
                let mid = 0.5 * (lo + hi);
                if (Self { c: mid, ..c }).in_gamut() {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            c.c = lo;
        }
        let lin = c.to_linear_srgb();
        Srgb {
            r: linear_to_srgb_u8(lin[0]),
            g: linear_to_srgb_u8(lin[1]),
            b: linear_to_srgb_u8(lin[2]),
        }
    }

    /// Whether the unclamped linear-sRGB value is inside the gamut (with a
    /// half-level of slack, so the 8-bit encoding of an edge colour counts).
    fn in_gamut(&self) -> bool {
        const EPS: f32 = 0.0005;
        self.linear_unclamped()
            .iter()
            .all(|&v| (-EPS..=1.0 + EPS).contains(&v))
    }

    fn linear_unclamped(&self) -> [f32; 3] {
        let h_rad = self.h.to_radians();
        let oklab_a = self.c * h_rad.cos();
        let oklab_b = self.c * h_rad.sin();

        let l_ = self.l + 0.3963377774 * oklab_a + 0.2158037573 * oklab_b;
        let m_ = self.l - 0.1055613458 * oklab_a - 0.0638541728 * oklab_b;
        let s_ = self.l - 0.0894841775 * oklab_a - 1.2914855480 * oklab_b;

        let l_cone = l_ * l_ * l_;
        let m_cone = m_ * m_ * m_;
        let s_cone = s_ * s_ * s_;

        [
            4.0767416621 * l_cone - 3.3077115913 * m_cone + 0.2309699292 * s_cone,
            -1.2684380046 * l_cone + 2.6097574011 * m_cone - 0.3413193965 * s_cone,
            -0.0041960863 * l_cone - 0.7034186147 * m_cone + 1.7076147010 * s_cone,
        ]
    }

    /// Convert to linear-sRGB straight-alpha [r, g, b, a] in [0..1].
    /// Out-of-gamut components are clamped to [0, 1].
    pub fn to_linear_srgb(&self) -> [f32; 4] {
        let [r, g, b] = self.linear_unclamped();
        [
            r.clamp(0.0, 1.0),
            g.clamp(0.0, 1.0),
            b.clamp(0.0, 1.0),
            self.a.clamp(0.0, 1.0),
        ]
    }

    /// Convert to gamma-encoded sRGB straight-alpha. Useful when a target
    /// API expects 8-bit `Srgba` instead of float.
    pub fn to_srgba(&self) -> Srgba {
        let lin = self.to_linear_srgb();
        Srgba {
            r: linear_to_srgb_u8(lin[0]),
            g: linear_to_srgb_u8(lin[1]),
            b: linear_to_srgb_u8(lin[2]),
            a: lin[3],
        }
    }

    /// Convert to sRGB-encoded float channels [r, g, b, a] in [0..1].
    ///
    /// Use this for renderers that expect sRGB-encoded floats (e.g. Skia's
    /// default `Color4f` interpretation). Passing raw linear values into an
    /// sRGB-encoded slot makes the color render visibly darker than intended.
    pub fn to_srgb_f32(&self) -> [f32; 4] {
        let lin = self.to_linear_srgb();
        [
            linear_to_srgb_f32(lin[0]),
            linear_to_srgb_f32(lin[1]),
            linear_to_srgb_f32(lin[2]),
            lin[3],
        ]
    }
}

#[inline]
fn srgb_u8_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb_f32(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn linear_to_srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let encoded = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Velvet token round-trips through OkLCH to within one 8-bit level.
    #[test]
    fn srgb_oklch_round_trip() {
        for hex in [
            0xE5B8C5, 0xB47491, 0xC9A5D4, 0xE8D4A8, 0xC96A7A, 0xF4E8EA, 0x9A8087, 0x0A0006,
            0x120010, 0x000000, 0xFFFFFF, 0x808080,
        ] {
            let c = Srgb::from_hex(hex);
            let back = OkLch::from_srgb(c).to_srgb();
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!(
                    (a as i32 - b as i32).abs() <= 1,
                    "{hex:06X}: {c:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn zero_shift_is_exact_identity() {
        let c = Srgb::from_hex(0xE5B8C5);
        assert_eq!(c.hue_shifted(0.0), c);
        let k = OkLch::new(0.85, 0.06, 340.0, 0.12);
        assert_eq!(k.rotate_hue(0.0), k);
        let a = Srgba::from_hex_a(0xB482A0, 0.06);
        assert_eq!(a.hue_shifted(0.0), a);
    }

    #[test]
    fn shift_rotates_hue_and_keeps_lightness() {
        let c = Srgb::from_hex(0xE5B8C5);
        let before = OkLch::from_srgb(c);
        let after = OkLch::from_srgb(c.hue_shifted(90.0));
        let dh = (after.h - before.h).rem_euclid(360.0);
        assert!((dh - 90.0).abs() < 6.0, "hue moved by {dh}");
        assert!((after.l - before.l).abs() < 0.02, "{before:?} -> {after:?}");
        assert!(c.hue_shifted(90.0) != c);
        // A full turn comes back to (nearly) the same colour.
        let full = c.hue_shifted(360.0);
        assert!((full.r as i32 - c.r as i32).abs() <= 1);
        assert!((full.g as i32 - c.g as i32).abs() <= 1);
        assert!((full.b as i32 - c.b as i32).abs() <= 1);
    }

    #[test]
    fn negative_shift_and_greys() {
        let c = Srgb::from_hex(0xB47491);
        assert!(c.hue_shifted(-120.0) != c);
        // A grey has no hue to rotate.
        let g = Srgb::from_hex(0x808080);
        let s = g.hue_shifted(137.0);
        assert!((s.r as i32 - g.r as i32).abs() <= 1 && (s.b as i32 - g.b as i32).abs() <= 1);
    }
}
