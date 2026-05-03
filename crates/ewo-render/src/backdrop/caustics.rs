//! Caustics — slow iridescent light-on-silk shimmer.
//!
//! CSS:
//! ```css
//! .caustics { mix-blend-mode: screen; }
//! .caustics-layer {
//!   inset: -20%;
//!   background:
//!     radial-gradient(40% 30% at 20% 30%, oklch(0.85 0.06 340 / 0.12), transparent 60%),
//!     radial-gradient(30% 40% at 70% 60%, oklch(0.82 0.08 300 / 0.1),  transparent 60%),
//!     radial-gradient(35% 35% at 50% 80%, oklch(0.88 0.05 45  / 0.08), transparent 60%);
//!   filter: blur(30px);
//!   animation: caustic-drift 38s linear infinite;
//! }
//! .caustics-layer-b {
//!   background:
//!     radial-gradient(35% 40% at 80% 20%, oklch(0.84 0.07 320 / 0.1), transparent 60%),
//!     radial-gradient(40% 30% at 30% 70%, oklch(0.86 0.06 25  / 0.08), transparent 60%);
//!   animation-duration: 52s;
//!   animation-direction: reverse;
//! }
//! @keyframes caustic-drift {
//!   to { transform: translate(3%, -4%) rotate(6deg); }
//! }
//! ```
//!
//! Each layer is rendered into a `save_layer` so we can apply the 30px blur
//! and screen-blend the result. The animation transform applies inside the
//! layer so the gradients drift together. `inset: -20%` is reproduced by
//! drawing each layer's gradients in an over-extended bounds, so when the
//! animation transform shifts them they don't expose the edge.

use ewo_core::OkLch;
use skia_safe::canvas::SaveLayerRec;
use skia_safe::{
    gradient_shader, image_filters, BlendMode, Canvas, Color4f, Matrix, Paint, Point, Rect,
    TileMode,
};

struct CausticBlob {
    /// Center as a fraction of layer width/height.
    center_pct: (f32, f32),
    /// Radius as a fraction of layer width/height (rx, ry).
    radius_pct: (f32, f32),
    color: OkLch,
}

const LAYER_A: &[CausticBlob] = &[
    CausticBlob {
        center_pct: (0.20, 0.30),
        radius_pct: (0.40, 0.30),
        color: OkLch::new(0.85, 0.06, 340.0, 0.12),
    },
    CausticBlob {
        center_pct: (0.70, 0.60),
        radius_pct: (0.30, 0.40),
        color: OkLch::new(0.82, 0.08, 300.0, 0.10),
    },
    CausticBlob {
        center_pct: (0.50, 0.80),
        radius_pct: (0.35, 0.35),
        color: OkLch::new(0.88, 0.05, 45.0, 0.08),
    },
];

const LAYER_B: &[CausticBlob] = &[
    CausticBlob {
        center_pct: (0.80, 0.20),
        radius_pct: (0.35, 0.40),
        color: OkLch::new(0.84, 0.07, 320.0, 0.10),
    },
    CausticBlob {
        center_pct: (0.30, 0.70),
        radius_pct: (0.40, 0.30),
        color: OkLch::new(0.86, 0.06, 25.0, 0.08),
    },
];

pub fn draw(canvas: &Canvas, w: f32, h: f32, time: f32) {
    // Layer A: 38s linear period.
    draw_layer(canvas, w, h, LAYER_A, (time / 38.0).fract());
    // Layer B: 52s, reversed direction (CSS `animation-direction: reverse`
    // means the keyframe is played from `to` → `from`, equivalent to negating
    // the phase).
    draw_layer(canvas, w, h, LAYER_B, 1.0 - (time / 52.0).fract());
}

fn draw_layer(canvas: &Canvas, w: f32, h: f32, blobs: &[CausticBlob], phase: f32) {
    // CSS `inset: -20%` over-extends the layer in all directions.
    const OVER: f32 = 0.20;
    let lw = w * (1.0 + 2.0 * OVER);
    let lh = h * (1.0 + 2.0 * OVER);
    let layer_origin_x = -OVER * w;
    let layer_origin_y = -OVER * h;

    // Animation transform: `to { transform: translate(3%, -4%) rotate(6deg) }`
    // applied at progress `phase`. The percentages are relative to the layer's
    // own size (per CSS transform spec).
    let tx = 0.03 * lw * phase;
    let ty = -0.04 * lh * phase;
    let rot_deg = 6.0 * phase;

    // 30px CSS blur → sigma 15.
    let blur = image_filters::blur((15.0, 15.0), TileMode::Decal, None, None)
        .expect("caustic blur filter");
    let mut layer_paint = Paint::default();
    layer_paint.set_image_filter(blur);
    layer_paint.set_blend_mode(BlendMode::Screen);

    let bounds = Rect::from_xywh(0.0, 0.0, w, h);
    let saved = canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint).bounds(&bounds));

    // Apply animation transform around the layer center.
    canvas.save();
    let center = (w * 0.5, h * 0.5);
    canvas.translate(center);
    canvas.rotate(rot_deg, None);
    canvas.translate((-center.0 + tx, -center.1 + ty));

    // Draw each blob as a gradient-painted full layer-rect.
    let layer_rect = Rect::from_xywh(layer_origin_x, layer_origin_y, lw, lh);
    for blob in blobs {
        draw_blob(canvas, &layer_rect, blob);
    }

    canvas.restore();
    canvas.restore_to_count(saved);
}

fn draw_blob(canvas: &Canvas, layer_rect: &Rect, blob: &CausticBlob) {
    let cx = layer_rect.left + layer_rect.width() * blob.center_pct.0;
    let cy = layer_rect.top + layer_rect.height() * blob.center_pct.1;
    let rx = layer_rect.width() * blob.radius_pct.0;
    let ry = layer_rect.height() * blob.radius_pct.1;
    let radius = rx.max(ry);

    let mut local = Matrix::default();
    local.pre_translate((cx, cy));
    local.pre_scale((rx / radius, ry / radius), None);
    local.pre_translate((-cx, -cy));

    // sRGB-encoded floats for Skia's default Color4f interpretation. Passing
    // raw linear-sRGB values here makes the caustics render visibly darker
    // than the browser does.
    let srgb = blob.color.to_srgb_f32();
    let inner = Color4f::new(srgb[0], srgb[1], srgb[2], srgb[3]);
    let outer = Color4f::new(srgb[0], srgb[1], srgb[2], 0.0);

    let shader = gradient_shader::radial(
        Point::new(cx, cy),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(&[inner, outer], None),
        Some(&[0.0_f32, 0.60][..]),
        TileMode::Clamp,
        None,
        Some(&local),
    )
    .expect("caustic blob shader");

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    canvas.draw_rect(*layer_rect, &paint);
}
