//! `glass_panel` — the canonical surface primitive.
//!
//! Composite of five layers, drawn in this order (the CSS z-stack puts the rims
//! at z=3, under the inner content at z=4, so the rims are drawn first):
//!   1. **glass-refract** — captures the current canvas through a 32px blur
//!      (CSS `backdrop-filter: blur(32px) saturate(1.45)`) and overlays a
//!      dark-mauve tint (`rgba(28,10,20,0.52)`).
//!   2. **glass-tint** — 135° linear gradient (rose → berry → lavender) plus
//!      a top-edge warm-white radial fade. Optional 8s opacity breath.
//!   3. **glass-rim** — four iridescent edge gradients (top/right/bottom/left)
//!      sliding on a 12s linear cycle with 3-second-staggered delays.
//!   4. **inner content** — caller-supplied via the `inner` closure, drawn
//!      with the panel rrect already clipped.
//!   5. **hairline rim** — 1px stroke at `rgba(229,184,197,0.18)` to crisp
//!      the rounded corners.
//!
//! CSS reference is in `StyleSheet1` from `.glass-panel` through
//! `@keyframes rim-slide-v`. The React component is `Panel` at line ~6760 of
//! the prototype HTML.
//!
//! Design rule (CLAUDE.md non-negotiable #2/#3): the tint layer is the
//! breath carrier — the panel itself never scales and never blurs, because
//! doing so would re-rasterize the text glyphs above it. The prototype's
//! `panel-mount` keyframe (scale 0.98 + blur 8px on the whole panel) is not
//! implemented here for the same reason. Screens that want an entrance fade
//! the panel with an alpha layer plus a translate rounded to whole *device*
//! pixels (`offscreen::snap_dy`; see the `settings.rs` tab changes) or
//! animate the chrome layers separately, as the modals do; the inner content
//! is never scaled and never sits on a fractional-pixel offset.

use crate::gradients;
use crate::tokens;
use ewo_core::{motion, Settings};
use skia_safe::{
    canvas::SaveLayerRec, image_filters, BlurStyle, Canvas, ClipOp, Color4f, MaskFilter, Matrix,
    Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

pub const PANEL_RADIUS: f32 = 18.0;

/// Draw a glass panel with caller-supplied inner content.
///
/// The `inner` closure receives the canvas with the panel rrect already
/// clipped — caller draws in card-local coordinates, anywhere inside `bounds`.
pub fn draw_glass_panel<F: FnOnce(&Canvas)>(
    canvas: &Canvas,
    bounds: Rect,
    breathing: bool,
    time: f32,
    settings: &Settings,
    inner: F,
) {
    let rrect = RRect::new_rect_xy(bounds, PANEL_RADIUS, PANEL_RADIUS);

    draw_refract(canvas, &rrect, &bounds);
    draw_tint(canvas, &rrect, &bounds, breathing, time, settings);
    // CSS z-stack: rims (z=3) under inner content (z=4). Drawing rims *before*
    // inner means any content drawn near the edge correctly paints over them.
    // The rim rects extend straight across the panel's full width/height, so
    // they MUST be clipped to the rrect or the rounded corners read as square.
    {
        let saved = canvas.save();
        canvas.clip_rrect(rrect, None, Some(true));
        draw_rims(canvas, &bounds, time, settings.motion_speed);
        canvas.restore_to_count(saved);
    }

    // Inner content — clipped to the panel's rrect so children can't escape.
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
    inner(canvas);
    canvas.restore_to_count(saved);

    draw_hairline(canvas, &bounds);
}

// ────────────────────────────────────────────────────────────────────────
// Layer 1 — glass-refract: backdrop blur + dark fill
// ────────────────────────────────────────────────────────────────────────

fn draw_refract(canvas: &Canvas, rrect: &RRect, bounds: &Rect) {
    // CSS `backdrop-filter: blur(32px) saturate(1.45)` — Skia approximates
    // this via a save_layer with a backdrop ImageFilter. The layer captures
    // the current canvas content (the backdrop layers + anything drawn
    // beneath this panel) through the filter; subsequent draws into the
    // layer composite on top. `saturate(1.45)` is omitted — the saturation
    // boost is subtle and the dark tint fill below dominates the result.
    let Some(blur) = image_filters::blur(
        // CSS blur(32) → sigma 16 (industry-standard /2 conversion).
        (16.0, 16.0),
        TileMode::Clamp,
        None,
        None,
    ) else {
        return;
    };

    let saved = canvas.save();
    canvas.clip_rrect(*rrect, Some(ClipOp::Intersect), Some(true));
    let rec = SaveLayerRec::default().bounds(bounds).backdrop(&blur);
    canvas.save_layer(&rec);

    // Dark fill on top of the now-blurred backdrop.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(tokens::bg_glass_fill_a(0.52), None);
    canvas.draw_rrect(*rrect, &fill);

    canvas.restore();
    canvas.restore_to_count(saved);
}

// ────────────────────────────────────────────────────────────────────────
// Layer 2 — glass-tint: 135° linear + top radial fade, optional breath
// ────────────────────────────────────────────────────────────────────────

fn draw_tint(
    canvas: &Canvas,
    rrect: &RRect,
    bounds: &Rect,
    breathing: bool,
    time: f32,
    settings: &Settings,
) {
    let breath_alpha = if breathing {
        let speed = settings.motion_speed.max(0.0001);
        // Reduced motion: `.glass-tint` rests on its base opacity 1 (phase 0).
        let phase = (motion::ambient_time(time) / (8.0 / speed)).rem_euclid(1.0);
        let triangle = if phase < 0.5 {
            phase * 2.0
        } else {
            (1.0 - phase) * 2.0
        };
        let smooth = triangle * triangle * (3.0 - 2.0 * triangle);
        // CSS keyframe: 1.0 ↔ (1.0 - 0.10*breath_amp), peak depth at phase=0.5.
        1.0 - 0.10 * settings.breath_amp * smooth
    } else {
        1.0
    };

    let saved = canvas.save();
    canvas.clip_rrect(*rrect, Some(ClipOp::Intersect), Some(true));

    // 135° linear gradient.
    if let Some(lin) = gradients::linear(
        (
            Point::new(bounds.left, bounds.top),
            Point::new(bounds.right, bounds.bottom),
        ),
        &[
            tokens::accent_rose_a(0.18),
            tokens::accent_berry_a(0.12),
            tokens::accent_lav_a(0.16),
        ],
        Some(&[0.0_f32, 0.40, 1.0][..]),
        TileMode::Clamp,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_alpha_f(breath_alpha);
        p.set_shader(lin);
        canvas.draw_rrect(*rrect, &p);
    }

    // Top radial fade. CSS: `radial-gradient(120% 80% at 50% -20%, warm-white
    // 0.14, transparent 60%)`. Skia's radial is circular; we apply a local
    // matrix to stretch into the elliptical 120%×80% ellipse.
    let cx = bounds.left + bounds.width() * 0.5;
    let cy = bounds.top + bounds.height() * -0.2;
    let base_r = bounds.width() * 0.6; // 60% of the 120% width as a circle
    let mut local = Matrix::default();
    local.pre_translate((cx, cy));
    local.pre_scale((1.0, (bounds.height() * 0.8) / base_r), None);
    local.pre_translate((-cx, -cy));
    if let Some(rad) = gradients::radial(
        Point::new(cx, cy),
        base_r,
        &[tokens::text_pearl_a(0.14), tokens::text_pearl_a(0.0)],
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        Some(&local),
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_alpha_f(breath_alpha);
        p.set_shader(rad);
        canvas.draw_rrect(*rrect, &p);
    }

    canvas.restore_to_count(saved);
}

// ────────────────────────────────────────────────────────────────────────
// Layer 4 — glass-rim: four edge gradients on a 12s slide cycle
// ────────────────────────────────────────────────────────────────────────

fn draw_rims(canvas: &Canvas, bounds: &Rect, time: f32, motion_speed: f32) {
    let speed = motion_speed.max(0.0001);
    let period = 12.0 / speed;

    // CSS background-position animates 0 → 240% over 12s linear. Phase 0..1
    // tracks that sweep. Delays per CSS: top 0, right -3s, bottom -6s, left -9s.
    //
    // Reduced motion (StyleSheet1 ~L1563): the loops stop and every rim rests
    // on its base `background-position: 0`; with no animation running, the
    // per-edge `animation-delay` offsets no longer apply either.
    let reduced = motion::is_reduced();
    let phase = |delay_sec: f32| {
        if reduced {
            0.0
        } else {
            ((time + delay_sec) / period).rem_euclid(1.0)
        }
    };

    let trans = tokens::accent_rose_a(0.0);
    let rose = tokens::accent_rose_a(1.0);
    let lav = tokens::accent_lav_a(1.0);
    let champ = tokens::accent_champ_a(1.0);

    // Top: rose, lav, champ, rose
    draw_horizontal_rim(
        canvas,
        Rect::from_xywh(bounds.left, bounds.top, bounds.width(), 1.5),
        phase(0.0),
        [trans, rose, lav, champ, rose, trans],
    );
    // Right: champ, rose, lav, champ
    draw_vertical_rim(
        canvas,
        Rect::from_xywh(bounds.right - 1.5, bounds.top, 1.5, bounds.height()),
        phase(3.0),
        [trans, champ, rose, lav, champ, trans],
    );
    // Bottom: lav, champ, rose, lav
    draw_horizontal_rim(
        canvas,
        Rect::from_xywh(bounds.left, bounds.bottom - 1.5, bounds.width(), 1.5),
        phase(6.0),
        [trans, lav, champ, rose, lav, trans],
    );
    // Left: rose, champ, lav, rose
    draw_vertical_rim(
        canvas,
        Rect::from_xywh(bounds.left, bounds.top, 1.5, bounds.height()),
        phase(9.0),
        [trans, rose, champ, lav, rose, trans],
    );
}

fn draw_horizontal_rim(canvas: &Canvas, rect: Rect, phase: f32, colors: [Color4f; 6]) {
    // CSS `background-size: 240% 100%` then animation moves background-position
    // by 240%. We model this as a 2.4×-wide gradient pattern panned across the
    // rim by `phase * pattern_w`, with TileMode::Repeat so the pattern wraps.
    let pattern_w = rect.width() * 2.4;
    let offset = phase * pattern_w;
    let p0 = rect.left - offset;
    let p1 = p0 + pattern_w;

    let Some(shader) = gradients::linear(
        (Point::new(p0, rect.top), Point::new(p1, rect.top)),
        &colors,
        Some(&[0.0_f32, 0.20, 0.40, 0.60, 0.80, 1.0][..]),
        TileMode::Repeat,
        None,
    ) else {
        return;
    };

    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 0.5, false));
    p.set_alpha_f(0.85); // CSS .glass-rim opacity
    canvas.draw_rect(rect, &p);
}

fn draw_vertical_rim(canvas: &Canvas, rect: Rect, phase: f32, colors: [Color4f; 6]) {
    let pattern_h = rect.height() * 2.4;
    let offset = phase * pattern_h;
    let p0 = rect.top - offset;
    let p1 = p0 + pattern_h;

    let Some(shader) = gradients::linear(
        (Point::new(rect.left, p0), Point::new(rect.left, p1)),
        &colors,
        Some(&[0.0_f32, 0.20, 0.40, 0.60, 0.80, 1.0][..]),
        TileMode::Repeat,
        None,
    ) else {
        return;
    };

    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 0.5, false));
    p.set_alpha_f(0.85);
    canvas.draw_rect(rect, &p);
}

// ────────────────────────────────────────────────────────────────────────
// Layer 5 — hairline rim
// ────────────────────────────────────────────────────────────────────────

fn draw_hairline(canvas: &Canvas, bounds: &Rect) {
    let inset = bounds.with_inset((0.5, 0.5));
    let rrect = RRect::new_rect_xy(inset, PANEL_RADIUS - 0.5, PANEL_RADIUS - 0.5);

    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(tokens::accent_rose_a(0.18), None);
    canvas.draw_rrect(rrect, &rim);
}
