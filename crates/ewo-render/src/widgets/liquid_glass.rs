//! Liquid glass — a refracting glass plate, drawn with an SkSL runtime shader.
//!
//! The existing `.iw-shell` in-game plate is a flat translucent wine fill: it
//! *tints* what is behind it but does not *bend* it, so it reads as a sticker
//! laid on the screen rather than a physical object above it. What sells real
//! glass is edge refraction — near the rim, a thick bevel displaces the
//! background along the surface normal, magnifying and smearing it, with a
//! grazing specular highlight riding the lip and a little chromatic dispersion
//! where the bend is strongest.
//!
//! All of that is one fragment shader over a signed-distance field:
//!
//! - `sdRoundBox` gives the distance to the rounded-rect edge. Negative inside.
//! - Central differences on that SDF give the outward surface normal, with no
//!   need for screen-space derivatives (which runtime effects can't rely on).
//! - A cubic falloff over the last `edge` pixels is the bevel profile: dead
//!   flat across the middle, bending hard only at the rim. This is what keeps
//!   text in the centre of a widget perfectly readable — the flat region is
//!   a straight backdrop sample, displaced by nothing.
//!
//! **The caller supplies the backdrop.** Pass an already-blurred snapshot for
//! frosted glass, a sharp one for clear glass. The shader deliberately does no
//! blurring of its own: in-game the blurred backdrop already exists as the
//! cached frost surface, and re-blurring per widget would be pure waste.
//!
//! Non-negotiable #2 ("don't transform anything that contains text") is not at
//! risk here — the glass is a backdrop treatment drawn *beneath* content. Text
//! is drawn afterwards, untransformed, at its natural raster size.

use std::cell::RefCell;

use skia_safe::{
    runtime_effect::ChildPtr, BlendMode, BlurStyle, Canvas, Color4f, Data, Image, MaskFilter,
    Matrix, Paint, RRect, RuntimeEffect, SamplingOptions, Shader, TileMode,
};

/// The SkSL. Uniform order here must match [`Params::to_uniform_bytes`].
///
/// Alignment: SkSL packs uniforms with std140-ish rules — `float2` on an
/// 8-byte boundary, `float4` on 16. The declaration order below is chosen so
/// every field lands naturally aligned with no padding, which is what lets the
/// Rust side write a flat little-endian buffer instead of querying offsets.
const GLASS_SKSL: &str = r#"
// Two backdrop sources, because they answer different questions.
//   `rim`   — lightly blurred. Refraction of featureless pixels is invisible,
//             so the bevel needs a source that still has structure to bend.
//   `frost` — heavily blurred. The flat interior is a content surface; text
//             has to sit on it, so whatever shows through must be mush.
// Blending them by the bevel profile gives a frosted plate with a live,
// structured edge — which is exactly what the reference does.
uniform shader rim;
uniform shader frost;

uniform float2 uCenter;    //  0
uniform float2 uHalf;      //  8
uniform float4 uTint;      // 16  (rgb + mix amount)
uniform float  uRadius;    // 32
uniform float  uEdge;      // 36  bevel width, px
uniform float  uStrength;  // 40  max refraction displacement, px
uniform float  uDisperse;  // 44  chromatic split at the rim, px
uniform float  uSpecular;  // 48  highlight gain
uniform float  uTime;      // 52  seconds — drives the liquid ripple
uniform float  uRipple;    // 56  ripple depth, 0 = perfectly still glass
uniform float  uHairline;  // 60  crisp edge-line gain

float sdRoundBox(float2 p, float2 b, float r) {
    float2 q = abs(p) - b + float2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, float2(0.0))) - r;
}

half4 main(float2 coord) {
    float2 p = coord - uCenter;
    float d = sdRoundBox(p, uHalf, uRadius);

    // Analytic anti-aliased coverage — one pixel of feather across the edge.
    float cov = clamp(0.5 - d, 0.0, 1.0);
    if (cov <= 0.0) {
        return half4(0.0);
    }

    // Outward normal, from central differences on the SDF.
    float e = 1.0;
    float2 n = float2(
        sdRoundBox(p + float2(e, 0.0), uHalf, uRadius)
            - sdRoundBox(p - float2(e, 0.0), uHalf, uRadius),
        sdRoundBox(p + float2(0.0, e), uHalf, uRadius)
            - sdRoundBox(p - float2(0.0, e), uHalf, uRadius)
    );
    float nl = length(n);
    n = nl > 0.0001 ? n / nl : float2(0.0, -1.0);

    // Bevel profile: 0 across the flat middle, 1 at the rim. Cubed so the
    // transition is soft and the centre stays genuinely undistorted.
    float t = clamp(1.0 + d / uEdge, 0.0, 1.0);
    float bend = t * t * t;

    // The "liquid" part — a slow standing wave travelling around the rim, so
    // the bevel breathes instead of sitting frozen. Purely a modulation of the
    // bend, so it can never distort the flat centre.
    float ang = atan(n.y, n.x);
    bend *= 1.0 + uRipple * sin(ang * 3.0 + uTime * 1.10)
                * (0.6 + 0.4 * sin(ang * 5.0 - uTime * 0.70));

    // Refraction. The sample is pushed *outward* along the normal, which
    // compresses a wide band of the surroundings into the narrow bevel — the
    // read of a thick lens edge gripping what is behind it. Sampling inward
    // instead merely smears the interior and is nearly invisible once the
    // backdrop is blurred at all.
    float2 off = n * (uStrength * bend);
    float2 dsp = n * (uDisperse * bend);
    half r = rim.eval(coord + off + dsp).r;
    half g = rim.eval(coord + off).g;
    half b = rim.eval(coord + off - dsp).b;
    half3 refracted = half3(r, g, b);

    // The flat interior is the frosted source, undisplaced. Cross-fade to the
    // refracted rim over the bevel.
    half3 interior = frost.eval(coord).rgb;
    half3 col = mix(interior, refracted, half(bend));

    // Velvet tint. Weighted toward the interior — the centre has to carry
    // text, the rim wants to stay glassy and let the world through.
    half tintAmt = half(uTint.a) * half(1.0 - 0.55 * bend);
    col = mix(col, half3(uTint.rgb), tintAmt);

    // ── Rim lighting ────────────────────────────────────────────────────
    // A key light from the upper-left plus a weaker bounce from the opposite
    // side, both confined to the bevel. Two lobes rather than one is what
    // stops the edge reading as a flat stroke.
    float2 L = normalize(float2(-0.55, -0.83));
    float rim = smoothstep(0.55, 1.0, t);
    float key = pow(max(dot(n, L), 0.0), 2.5) * rim;
    float bounce = pow(max(dot(n, -L), 0.0), 5.0) * rim;
    col += half3(half(uSpecular * (key * 2.4 + bounce * 0.8)));

    // A crisp hairline riding the outer edge — this is what actually draws the
    // shape. Without it the plate has no silhouette and dissolves into busy
    // backdrops.
    float hair = 1.0 - smoothstep(0.0, 1.6, abs(d + 1.0));
    col += half3(half(uHairline * hair * (0.35 + 0.65 * max(dot(n, L), 0.0))));

    // Darkening just inside the lip reads as glass thickness, and separates
    // the bright rim from the flat interior.
    col *= half(1.0 - smoothstep(0.30, 0.95, t) * 0.18);

    // Premultiplied.
    return half4(col * half(cov), half(cov));
}
"#;

thread_local! {
    /// Compiled once per thread. Runtime-effect compilation is not free, and
    /// re-compiling inside a per-frame draw is exactly the "foreign allocation
    /// in a draw call" mistake the 2026-05-31 perf pass was about — see the
    /// CLAUDE.md "Memory + performance pass" section.
    static EFFECT: RefCell<Option<RuntimeEffect>> = const { RefCell::new(None) };
}

fn with_effect<R>(f: impl FnOnce(&RuntimeEffect) -> R) -> Option<R> {
    EFFECT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            match RuntimeEffect::make_for_shader(GLASS_SKSL, None) {
                Ok(effect) => *slot = Some(effect),
                Err(err) => {
                    log::warn!("liquid_glass: SkSL failed to compile: {err}");
                    return None;
                }
            }
        }
        slot.as_ref().map(f)
    })
}

/// Tuning for one glass plate.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Corner radius, px.
    pub radius: f32,
    /// Width of the refracting bevel, px. Everything further in than this is
    /// flat, undistorted glass. Keep it well under half the plate's short side
    /// or the whole surface warps and text behind it swims.
    pub edge: f32,
    /// Peak displacement at the rim, px. This is the knob that reads as
    /// "thickness".
    pub strength: f32,
    /// Chromatic split at the rim, px. Small values (0.5–2) sell real glass;
    /// large values read as a bug.
    pub disperse: f32,
    /// Specular gain on the rim highlight.
    pub specular: f32,
    /// Tint colour and how strongly it is mixed over the refracted backdrop.
    pub tint: Color4f,
    /// Ripple depth — 0 is still glass, ~0.25 is a gentle liquid breathe.
    pub ripple: f32,
    /// Gain of the crisp edge hairline. This is what gives the plate a
    /// silhouette over busy backdrops; without it the shape dissolves.
    pub hairline: f32,
    /// Drop-shadow opacity beneath the plate. 0 disables it.
    pub shadow: f32,
}

impl Params {
    /// The in-world HUD widget plate: Velvet-tinted, restrained motion.
    pub const WIDGET: Params = Params {
        radius: 14.0,
        edge: 16.0,
        strength: 11.0,
        disperse: 1.1,
        specular: 0.16,
        // Wine, mixed lightly — enough to belong to the theme, not enough to
        // hide what is behind it. The flat 0.50 fill this replaces is what
        // made the old plate read as washed out.
        // Wine, mixed at the interior. The shader thins this toward the rim,
        // so the centre stays a readable content surface while the edge stays
        // glassy — one number can't do both jobs, which is why the old flat
        // 0.50 fill had to choose "readable" and lost the glass.
        tint: Color4f::new(0.094, 0.0, 0.055, 0.52),
        ripple: 0.22,
        hairline: 0.34,
        shadow: 0.55,
    };

    /// A larger dashboard surface — wider bevel, calmer.
    pub const PANEL: Params = Params {
        radius: 22.0,
        edge: 26.0,
        strength: 16.0,
        disperse: 1.4,
        specular: 0.14,
        tint: Color4f::new(0.094, 0.0, 0.055, 0.56),
        ripple: 0.16,
        hairline: 0.30,
        shadow: 0.62,
    };

    pub fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
    pub fn with_ripple(mut self, ripple: f32) -> Self {
        self.ripple = ripple;
        self
    }
    pub fn with_tint(mut self, tint: Color4f) -> Self {
        self.tint = tint;
        self
    }

    /// Flat little-endian uniform buffer, in declaration order. See the
    /// alignment note on [`GLASS_SKSL`].
    fn to_uniform_bytes(self, center: (f32, f32), half: (f32, f32), time: f32) -> Vec<u8> {
        let mut out = Vec::with_capacity(60);
        let mut put = |v: f32| out.extend_from_slice(&v.to_le_bytes());
        put(center.0);
        put(center.1);
        put(half.0);
        put(half.1);
        put(self.tint.r);
        put(self.tint.g);
        put(self.tint.b);
        put(self.tint.a);
        put(self.radius);
        put(self.edge);
        put(self.strength);
        put(self.disperse);
        put(self.specular);
        put(time);
        put(self.ripple);
        put(self.hairline);
        out
    }
}

/// The two backdrop snapshots the shader samples, in **canvas coordinates** —
/// both must be full-frame images, not crops, because the shader samples at
/// absolute positions. `*_scale` is each image's size relative to the canvas
/// (`0.5` for a half-resolution snapshot, `1.0` for full).
#[derive(Clone, Copy)]
pub struct Backdrop<'a> {
    /// Lightly-blurred — what the refracting bevel bends.
    pub rim: &'a Image,
    pub rim_scale: f32,
    /// Heavily-blurred — what shows through the flat interior. In-game this is
    /// the cached quarter-resolution frost surface, already being maintained
    /// for the overlay, so reusing it here is free.
    pub frost: &'a Image,
    pub frost_scale: f32,
}

impl<'a> Backdrop<'a> {
    /// Use one image for both roles. Cheaper, but the rim and the interior then
    /// want incompatible amounts of blur — fine for a quick call, not the look.
    pub fn uniform(image: &'a Image, scale: f32) -> Self {
        Backdrop { rim: image, rim_scale: scale, frost: image, frost_scale: scale }
    }
}

/// Build an image shader that samples in canvas space, compensating for a
/// snapshot taken at reduced resolution.
fn canvas_space_shader(image: &Image, scale: f32) -> Option<Shader> {
    let inv = if scale > 0.0 { 1.0 / scale } else { 1.0 };
    let local = Matrix::scale((inv, inv));
    image.to_shader(
        (TileMode::Clamp, TileMode::Clamp),
        SamplingOptions::default(),
        Some(&local),
    )
}

/// Draw a refracting glass plate filling `bounds`.
///
/// Returns `false` if the shader could not be built, so callers can fall back
/// to a flat plate rather than drawing nothing.
#[must_use]
pub fn draw_liquid_glass(
    canvas: &Canvas,
    bounds: skia_safe::Rect,
    backdrop: Backdrop<'_>,
    params: Params,
    time: f32,
) -> bool {
    let (Some(rim), Some(frost)) = (
        canvas_space_shader(backdrop.rim, backdrop.rim_scale),
        canvas_space_shader(backdrop.frost, backdrop.frost_scale),
    ) else {
        return false;
    };

    let center = (bounds.center_x(), bounds.center_y());
    let half = (bounds.width() * 0.5, bounds.height() * 0.5);
    let uniforms = Data::new_copy(&params.to_uniform_bytes(center, half, time));

    let shader: Option<Shader> = with_effect(|effect| {
        effect.make_shader(
            uniforms,
            &[ChildPtr::Shader(rim), ChildPtr::Shader(frost)],
            None,
        )
    })
    .flatten();

    let Some(shader) = shader else {
        return false;
    };

    // Drop shadow first — the plate has to sit *above* the world, and over a
    // bright backdrop the refraction alone gives no separation.
    if params.shadow > 0.0 {
        let mut shadow = Paint::default();
        shadow.set_anti_alias(true);
        shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, params.shadow), None);
        shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 9.0, false));
        canvas.draw_rrect(
            RRect::new_rect_xy(bounds.with_offset((0.0, 6.0)), params.radius, params.radius),
            &shadow,
        );
    }

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    // The shader writes its own coverage, so blend it normally over whatever is
    // beneath — in-game that is the transparent offscreen, in the launcher the
    // backdrop itself.
    paint.set_blend_mode(BlendMode::SrcOver);
    // Draw the shape rather than the bounding box: the SDF already clips, but
    // constraining the raster area keeps the fragment cost proportional to the
    // plate, not its bounding rect.
    canvas.draw_rrect(
        RRect::new_rect_xy(bounds, params.radius, params.radius),
        &paint,
    );
    true
}
