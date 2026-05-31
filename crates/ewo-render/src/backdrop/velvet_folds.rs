//! Velvet folds — three slow-drifting oklch radial gradient layers,
//! blurred 40px, screen-blended at 85% opacity. The dominant color
//! contributor of the backdrop.
//!
//! React reference:
//! ```jsx
//! function VelvetFolds({ speed = 1, warmth = 0.6 }) {
//!   const hueA = 345 - warmth * 20; // ~333
//!   const hueB = 320 - warmth * 30; // ~302
//!   const hueC = 35 + warmth * 10;  // ~41
//!   // 3 vf-layer divs each with a single radial-gradient + animation-duration 22s/28s/34s
//! }
//! ```
//!
//! CSS keyframe `fold-drift` is `ease-in-out infinite alternate`:
//! ```text
//! 0%   { translate(-3%, -2%) scale(1.08) rotate(-1deg) }
//! 50%  { translate(2%, 3%)   scale(1.12) rotate(0.5deg) }
//! 100% { translate(-2%, -3%) scale(1.06) rotate(-0.5deg) }
//! ```
//!
//! Turbulence + displacement port: the layer's image filter is now
//! `blur(40px) → displacement_map(scale=28, source=fractalNoise(0.007 0.011,
//! octaves=2, seed=4))`, matching the SVG `feTurbulence` + `feDisplacementMap`
//! pair the prototype uses. Static turbulence (per author's perf rule —
//! animating the noise field per-frame was a full re-rasterize cost for
//! negligible additional movement; the silky-fold motion comes from the
//! gradient drift transforms below, not the noise).

use std::cell::RefCell;

use ewo_core::{OkLch, Settings};
use skia_safe::canvas::SaveLayerRec;
use skia_safe::{
    gradient_shader, image_filters, shaders, BlendMode, Canvas, Color4f, ColorChannel, ImageFilter,
    Matrix, Paint, Point, Rect, TileMode,
};

thread_local! {
    /// The layer's filter chain — `blur(40px) → displacement_map(scale=28,
    /// src=fractalNoise)` — is *fully static*: fixed seed/frequencies/octaves,
    /// fixed displacement scale, fixed blur sigma, and independent of canvas
    /// size, time, and settings. Building it per frame churned a fresh
    /// `SkPerlinNoiseShader` every frame, whose precomputed permutation/noise
    /// tables live in plain C++ heap that is invisible to Skia's resource and
    /// font caches — i.e. foreign memory that accumulated at ~500 fps. Build
    /// it once and clone the refcounted handle each frame (a cheap refcount
    /// bump). This was the dominant per-frame foreign-memory leak.
    static VELVET_FILTER: RefCell<Option<ImageFilter>> = const { RefCell::new(None) };
}

/// The cached, static velvet-folds image filter (see `VELVET_FILTER`).
fn velvet_filter() -> ImageFilter {
    VELVET_FILTER.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(build_velvet_filter)
            .clone()
    })
}

fn build_velvet_filter() -> ImageFilter {
    // blur(40px) → displacement(scale=28, src=noise).
    //
    // Mirrors `filter: blur(40px) url(#vfTurb)` where `vfTurb` is
    //   feTurbulence type=fractalNoise baseFrequency="0.007 0.011" octaves=2 seed=4
    //   feDisplacementMap in=SourceGraphic scale=28
    let blur = image_filters::blur((20.0, 20.0), TileMode::Decal, None, None)
        .expect("velvet-folds blur filter");

    let noise = shaders::fractal_noise((0.007, 0.011), 2, 4.0, None)
        .expect("fractal noise shader");
    let noise_filter = image_filters::shader(noise, None).expect("noise → filter");

    image_filters::displacement_map(
        (ColorChannel::R, ColorChannel::G),
        28.0,
        Some(noise_filter),
        Some(blur),
        None,
    )
    .expect("displacement_map filter")
}

struct Fold {
    /// Radial-gradient size as % of layer (rx, ry).
    radius_pct: (f32, f32),
    /// Center as % of layer (cx, cy).
    center_pct: (f32, f32),
    /// Inner color (CSS oklch). The gradient ramps from this color to
    /// transparent at the 70% stop (CSS uses `transparent 70%`).
    color: OkLch,
    /// Per-fold opacity multiplier (CSS `.vf-c { opacity: 0.6 }`).
    opacity: f32,
    /// Animation period in seconds (`fold-drift` duration).
    period_sec: f32,
    /// Phase offset in seconds (CSS `animation-delay`).
    delay_sec: f32,
}

/// Compute the three folds with the prototype's default `warmth = 0.6`.
fn folds(warmth: f32) -> [Fold; 3] {
    let hue_a = 345.0 - warmth * 20.0;
    let hue_b = 320.0 - warmth * 30.0;
    let hue_c = 35.0 + warmth * 10.0;

    [
        Fold {
            radius_pct: (0.60, 0.50),
            center_pct: (0.30, 0.40),
            color: OkLch::new(0.26, 0.08, hue_a, 0.90),
            opacity: 1.0,
            period_sec: 22.0,
            delay_sec: 0.0,
        },
        Fold {
            radius_pct: (0.55, 0.55),
            center_pct: (0.75, 0.65),
            color: OkLch::new(0.22, 0.09, hue_b, 0.85),
            opacity: 1.0,
            period_sec: 28.0,
            delay_sec: -8.0,
        },
        Fold {
            radius_pct: (0.40, 0.50),
            center_pct: (0.60, 0.20),
            color: OkLch::new(0.28, 0.05, hue_c, 0.55),
            opacity: 0.6,
            period_sec: 34.0,
            delay_sec: -14.0,
        },
    ]
}

pub fn draw(canvas: &Canvas, w: f32, h: f32, time: f32, settings: &Settings) {
    // Layer-level filter chain (static — built once, cloned per frame).
    let displaced = velvet_filter();

    let mut layer_paint = Paint::default();
    layer_paint.set_image_filter(displaced);
    layer_paint.set_blend_mode(BlendMode::Screen);
    layer_paint.set_alpha_f(0.85);

    let bounds = Rect::from_xywh(0.0, 0.0, w, h);
    let saved = canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint).bounds(&bounds));

    for fold in folds(settings.warmth).iter() {
        draw_fold(canvas, w, h, time, fold, settings.motion_speed);
    }

    canvas.restore_to_count(saved);
}

fn draw_fold(canvas: &Canvas, w: f32, h: f32, time: f32, fold: &Fold, motion_speed: f32) {
    // ease-in-out alternate over `period_sec / motion_speed`. CSS `alternate`
    // means the keyframe plays forward then backward — a triangle wave on the
    // 0..1 progress, with each half eased.
    let scaled_period = fold.period_sec / motion_speed;
    let phase = ((time + fold.delay_sec) / scaled_period).rem_euclid(2.0);
    let triangle = if phase < 1.0 { phase } else { 2.0 - phase };
    let eased = ease_in_out(triangle);

    let (tx_pct, ty_pct, sc, rot_deg) = lerp_fold_drift(eased);

    canvas.save();
    let cx = w * 0.5;
    let cy = h * 0.5;
    canvas.translate((cx, cy));
    canvas.rotate(rot_deg, None);
    canvas.scale((sc, sc));
    canvas.translate((tx_pct * 0.01 * w, ty_pct * 0.01 * h));
    canvas.translate((-cx, -cy));

    // CSS `inset: -20%` over-extends the layer so the animated transform
    // doesn't expose edges.
    const OVER: f32 = 0.20;
    let lw = w * (1.0 + 2.0 * OVER);
    let lh = h * (1.0 + 2.0 * OVER);
    let layer_rect = Rect::from_xywh(-OVER * w, -OVER * h, lw, lh);

    // Build the radial-gradient.
    let cx2 = layer_rect.left + lw * fold.center_pct.0;
    let cy2 = layer_rect.top + lh * fold.center_pct.1;
    let rx = lw * fold.radius_pct.0;
    let ry = lh * fold.radius_pct.1;
    let radius = rx.max(ry);

    let mut local = Matrix::default();
    local.pre_translate((cx2, cy2));
    local.pre_scale((rx / radius, ry / radius), None);
    local.pre_translate((-cx2, -cy2));

    let srgb = fold.color.to_srgb_f32();
    let inner = Color4f::new(srgb[0], srgb[1], srgb[2], srgb[3]);
    let outer = Color4f::new(srgb[0], srgb[1], srgb[2], 0.0);

    let shader = gradient_shader::radial(
        Point::new(cx2, cy2),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(&[inner, outer], None),
        Some(&[0.0_f32, 0.70][..]),
        TileMode::Clamp,
        None,
        Some(&local),
    )
    .expect("velvet-folds blob shader");

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    paint.set_alpha_f(fold.opacity);
    // The CSS prototype screen-blends each `.vf-layer` separately onto its
    // parent. Drawing them with default SrcOver inside our save_layer would
    // alpha-blend them together first (which dims the result by the layer
    // overlap area). Screen-blending each fold inside the layer matches the
    // spec — each fold ADDS light rather than averaging in.
    paint.set_blend_mode(BlendMode::Screen);
    canvas.draw_rect(layer_rect, &paint);

    canvas.restore();
}

/// Cubic-bezier(0.42, 0, 0.58, 1) approximation. CSS `ease-in-out` is exactly
/// that curve; smoothstep is close enough for slow ambient motion.
#[inline]
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interpolate the fold-drift keyframes (0% / 50% / 100%).
fn lerp_fold_drift(t: f32) -> (f32, f32, f32, f32) {
    if t < 0.5 {
        let s = t * 2.0;
        (
            lerp(-3.0, 2.0, s),
            lerp(-2.0, 3.0, s),
            lerp(1.08, 1.12, s),
            lerp(-1.0, 0.5, s),
        )
    } else {
        let s = (t - 0.5) * 2.0;
        (
            lerp(2.0, -2.0, s),
            lerp(3.0, -3.0, s),
            lerp(1.12, 1.06, s),
            lerp(0.5, -0.5, s),
        )
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
