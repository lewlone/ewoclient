//! Color types. sRGB linear, sRGB encoded, oklch.
//!
//! The prototype CSS uses oklch for several gradients (caustics, velvet folds,
//! petals) "for consistent luminance". We need oklch → linear-sRGB conversion
//! so gradient interpolation matches what the browser does.
//!
//! Conversion follows CSS Color Module Level 4 §11.3 (OKLab matrix).

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

impl Srgba {
    pub const fn from_hex_a(rgb: u32, a: f32) -> Self {
        let c = Srgb::from_hex(rgb);
        Self { r: c.r, g: c.g, b: c.b, a }
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

    /// Convert to linear-sRGB straight-alpha [r, g, b, a] in [0..1].
    /// Out-of-gamut components are clamped to [0, 1].
    pub fn to_linear_srgb(&self) -> [f32; 4] {
        // oklch → oklab
        let h_rad = self.h.to_radians();
        let oklab_a = self.c * h_rad.cos();
        let oklab_b = self.c * h_rad.sin();

        // oklab → LMS' (linear cone responses, cube-rooted)
        let l_ = self.l + 0.3963377774 * oklab_a + 0.2158037573 * oklab_b;
        let m_ = self.l - 0.1055613458 * oklab_a - 0.0638541728 * oklab_b;
        let s_ = self.l - 0.0894841775 * oklab_a - 1.2914855480 * oklab_b;

        // LMS' → LMS (cube)
        let l_cone = l_ * l_ * l_;
        let m_cone = m_ * m_ * m_;
        let s_cone = s_ * s_ * s_;

        // LMS → linear sRGB (matrix multiply)
        let r =  4.0767416621 * l_cone - 3.3077115913 * m_cone + 0.2309699292 * s_cone;
        let g = -1.2684380046 * l_cone + 2.6097574011 * m_cone - 0.3413193965 * s_cone;
        let b = -0.0041960863 * l_cone - 0.7034186147 * m_cone + 1.7076147010 * s_cone;

        [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), self.a.clamp(0.0, 1.0)]
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
