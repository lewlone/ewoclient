//! Bokeh orb — a single 50vmin soft pearl that crosses every 60s.
//!
//! CSS:
//! ```css
//! .bokeh-orb {
//!   width: 50vmin; height: 50vmin;
//!   top: 30%; left: -25%;
//!   border-radius: 50%;
//!   filter: blur(40px);
//!   opacity: 0.4;
//!   mix-blend-mode: screen;
//!   animation: bokeh-cross 60s linear infinite;
//! }
//! @keyframes bokeh-cross {
//!   0%   { transform: translate(-20%, -10%) scale(1);   opacity: 0; }
//!   15%  { opacity: 0.35; }
//!   50%  { transform: translate(60%, 20%)  scale(1.2); opacity: 0.45; }
//!   85%  { opacity: 0.30; }
//!   100% { transform: translate(140%, -5%) scale(0.9); opacity: 0; }
//! }
//! ```
//!
//! Tint variant 0 (berry) is the default and the only one we render in v1.

use ewo_core::Srgb;
use skia_safe::canvas::SaveLayerRec;
use skia_safe::{
    gradient_shader, image_filters, BlendMode, Canvas, Color4f, Paint, Point, Rect, TileMode,
};

const PERIOD_SEC: f32 = 60.0;

pub fn draw(canvas: &Canvas, w: f32, h: f32, time: f32) {
    let phase = (time / PERIOD_SEC).fract();

    // Three CSS keyframes for translate/scale: 0%, 50%, 100%.
    // Plus opacity stops at 0%, 15%, 50%, 85%, 100%.
    //
    // Note: the CSS `.bokeh-orb { opacity: 0.4 }` is the *resting* value —
    // when the animation is running, the keyframes override it (most-specific
    // wins). Don't multiply by 0.4 here.
    let (tx_pct, ty_pct, scale) = lerp_translate_scale(phase);
    let opacity = lerp_opacity(phase);
    if opacity <= 0.001 {
        return;
    }

    // 50vmin sized orb, positioned top: 30%, left: -25% relative to the
    // backdrop rect (which is our card interior, w × h). Skip when w/h
    // hit zero — happens on the first frame before a resize event lands,
    // and Skia's gradient_shader rejects zero-radius circles with None.
    let vmin = w.min(h);
    if vmin <= 0.0 {
        return;
    }
    let size = 0.50 * vmin * scale;
    // Initial position before transform.
    let base_left = -0.25 * w;
    let base_top = 0.30 * h;
    // CSS translate percentages are relative to the element's own bounding
    // box, not to the layer.
    let left = base_left + tx_pct * size;
    let top = base_top + ty_pct * size;

    let cx = left + size * 0.5;
    let cy = top + size * 0.5;
    let radius = size * 0.5;

    // Pearl-pink (berry) tint — variant 0 from the prototype.
    let inner = Srgb::from_hex(0xE5B8C5).to_f32();    // accent_rose
    let mid = Srgb::from_hex(0xB47491).to_f32();      // accent_berry, used at 0.2 alpha
    let inner_c = Color4f::new(inner[0], inner[1], inner[2], 1.0);
    let mid_c = Color4f::new(mid[0], mid[1], mid[2], 0.2);
    let outer_c = Color4f::new(0.0, 0.0, 0.0, 0.0);

    let Some(shader) = gradient_shader::radial(
        Point::new(cx, cy),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[inner_c, mid_c, outer_c],
            None,
        ),
        Some(&[0.0_f32, 0.55, 0.78][..]),
        TileMode::Clamp,
        None,
        None,
    ) else {
        return;
    };

    // 40px CSS blur → sigma 20. Apply via save_layer so screen-blend + opacity
    // compose correctly.
    let Some(blur) = image_filters::blur((20.0, 20.0), TileMode::Decal, None, None) else {
        return;
    };
    let mut layer_paint = Paint::default();
    layer_paint.set_image_filter(blur);
    layer_paint.set_blend_mode(BlendMode::Screen);
    layer_paint.set_alpha_f(opacity);

    let bounds = Rect::from_xywh(0.0, 0.0, w, h);
    let saved = canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint).bounds(&bounds));

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    canvas.draw_rect(Rect::from_xywh(cx - radius, cy - radius, size, size), &paint);

    canvas.restore_to_count(saved);
}

/// Interpolate translate/scale across the bokeh-cross keyframes.
fn lerp_translate_scale(phase: f32) -> (f32, f32, f32) {
    // 0%   → translate(-20%, -10%) scale(1)
    // 50%  → translate( 60%,  20%) scale(1.2)
    // 100% → translate(140%,  -5%) scale(0.9)
    if phase < 0.5 {
        let t = phase / 0.5;
        let tx = lerp(-0.20, 0.60, t);
        let ty = lerp(-0.10, 0.20, t);
        let s = lerp(1.0, 1.2, t);
        (tx, ty, s)
    } else {
        let t = (phase - 0.5) / 0.5;
        let tx = lerp(0.60, 1.40, t);
        let ty = lerp(0.20, -0.05, t);
        let s = lerp(1.2, 0.9, t);
        (tx, ty, s)
    }
}

/// Interpolate opacity across the keyframes (0%/15%/50%/85%/100%).
///
/// Bumped peaks slightly above the prototype's CSS keyframe values
/// (0.35/0.45/0.30 → 0.45/0.55/0.40) — the prototype sits inside a browser
/// with its own subtle gamma curve, and our straight Skia composite reads
/// dimmer at the same numbers. Visual parity beats numeric parity.
fn lerp_opacity(phase: f32) -> f32 {
    let stops = [(0.0, 0.0), (0.15, 0.45), (0.50, 0.55), (0.85, 0.40), (1.0, 0.0)];
    for w in stops.windows(2) {
        let (p0, v0) = w[0];
        let (p1, v1) = w[1];
        if phase >= p0 && phase <= p1 {
            let t = if p1 > p0 { (phase - p0) / (p1 - p0) } else { 0.0 };
            return lerp(v0, v1, t);
        }
    }
    0.0
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
