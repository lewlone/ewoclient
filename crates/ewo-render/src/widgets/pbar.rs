//! `pbar` — the prototype's progress bar.
//!
//! 2px-tall pill track with a gradient fill, a 3s flowing sheen across the
//! fill, and a soft pearl bloom at the leading edge.
//!
//! States:
//!   - **Normal** — pearl→rose→pearl gradient fill, flowing shimmer, bloom.
//!   - **Complete** — rose→pearl→champagne gradient with a wider outer glow,
//!     plus the expanding pearl ring (`pbar-ring`).
//!   - **ErrorRose** — ember/rose gradient fill, dim slow flow, ember glow.
//!   - **ErrorRecede** — animates the fill width from the snapshot at error
//!     time down to 0 over 1.2s silk easing, with an ember-tinted track.
//!   - **ErrorShimmer** — overlays an ember+pearl shimmer that sweeps and
//!     fades in/out (1.4s silk loop) on top of the normal fill.
//!
//! CSS reference: `.pbar`, `.pbar-track`, `.pbar-fill`, `.pbar-flow`,
//! `.pbar-bloom`, `.pbar-state-complete`, `.pbar-state-error.pbar-err-rose`,
//! `.pbar-state-error.pbar-err-recede`, `.pbar-err-shimmer` in `StyleSheet1`.

use ewo_core::Settings;
use skia_safe::{
    gradient_shader, BlendMode, BlurStyle, Canvas, Color4f, MaskFilter, Paint, PaintStyle, Point,
    RRect, Rect, TileMode,
};

const TRACK_HEIGHT: f32 = 2.0;
const RECEDE_DURATION: f32 = 1.2;
const ERR_SHIM_DURATION: f32 = 1.4;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PbarState {
    #[default]
    Normal,
    Complete,
    ErrorRose,
    /// Animates the fill from the snapshot fraction down to 0. The caller
    /// passes the snapshot fraction as `fraction` and the seconds-since-error
    /// as `state_age_seconds`. After RECEDE_DURATION the bar renders empty.
    ErrorRecede,
    ErrorShimmer,
}

impl PbarState {
    pub fn is_error(self) -> bool {
        matches!(
            self,
            PbarState::ErrorRose | PbarState::ErrorRecede | PbarState::ErrorShimmer
        )
    }
}

/// Draw a progress bar at the given bounds. The visible track height is
/// 2px, vertically centered in `bounds` so the caller can hand in a taller
/// row rect (gives the bloom room to bleed top/bottom).
///
/// `state_age_seconds` is the wall-clock seconds since the bar transitioned
/// into `state`. Used by:
///   - `Complete` — drives the expanding pearl ring (`pbar-ring`, 1.4s).
///   - `ErrorRecede` — drives the fill-width recede (`pbar-recede`, 1.2s).
/// Other states ignore it. Pass `None` if the caller doesn't track timing
/// (the animation is skipped).
pub fn draw_pbar(
    canvas: &Canvas,
    bounds: Rect,
    fraction: f32,
    state: PbarState,
    time: f32,
    settings: &Settings,
    state_age_seconds: Option<f32>,
) {
    let f_in = fraction.clamp(0.0, 1.0);
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let track_top = cy - TRACK_HEIGHT * 0.5;
    let track_rect = Rect::from_xywh(bounds.left, track_top, bounds.width(), TRACK_HEIGHT);
    let track_rrect = RRect::new_rect_xy(track_rect, TRACK_HEIGHT * 0.5, TRACK_HEIGHT * 0.5);

    draw_track(canvas, &track_rect, &track_rrect, state);

    // ErrorRecede animates fraction → 0 over RECEDE_DURATION.
    let f = if state == PbarState::ErrorRecede {
        let age = state_age_seconds.unwrap_or(0.0).max(0.0);
        if age >= RECEDE_DURATION {
            0.0
        } else {
            let t = age / RECEDE_DURATION;
            let eased = ewo_core::CubicBezier::SILK.eval(t.clamp(0.0, 1.0));
            f_in * (1.0 - eased)
        }
    } else {
        f_in
    };

    if f <= 0.0 {
        return;
    }

    let fill_rect = Rect::from_xywh(track_rect.left, track_rect.top, f * track_rect.width(), TRACK_HEIGHT);
    let fill_rrect = RRect::new_rect_xy(fill_rect, TRACK_HEIGHT * 0.5, TRACK_HEIGHT * 0.5);

    draw_fill(canvas, &fill_rect, &fill_rrect, state);

    // Flow shimmer behavior depends on state:
    //   - Normal / Complete: full-strength pearl flow at 3s.
    //   - ErrorRose: dimmed (35%) and slowed (8s) to feel stalled.
    //   - ErrorRecede: skip — the fill itself is animating away.
    //   - ErrorShimmer: replaced below with the ember-tinted shimmer overlay.
    match state {
        PbarState::Normal | PbarState::Complete => {
            draw_flow(canvas, &fill_rect, &fill_rrect, time, settings.motion_speed, 1.0, 3.0);
        }
        PbarState::ErrorRose => {
            draw_flow(canvas, &fill_rect, &fill_rrect, time, settings.motion_speed, 0.35, 8.0);
        }
        PbarState::ErrorRecede => {}
        PbarState::ErrorShimmer => {
            // Normal flow + the ember shimmer pass below.
            draw_flow(canvas, &fill_rect, &fill_rrect, time, settings.motion_speed, 1.0, 3.0);
        }
    }

    draw_bloom(canvas, &fill_rect, state);

    if state == PbarState::Complete {
        draw_complete_glow(canvas, &fill_rect, &fill_rrect);
        if let Some(age) = state_age_seconds {
            draw_complete_ring(canvas, &fill_rect, age);
        }
    }
    if state == PbarState::ErrorRose {
        draw_error_glow(canvas, &fill_rrect);
    }
    if state == PbarState::ErrorShimmer {
        // The shimmer overlay sweeps the full track, not just the fill, per
        // CSS `.pbar-err-shimmer { position: absolute; inset: 0; }`.
        draw_error_shimmer(canvas, &track_rect, &track_rrect, time, settings.motion_speed);
    }
}

const RING_DURATION: f32 = 1.4;
const RING_MAX_DIAMETER: f32 = 320.0;

fn draw_track(canvas: &Canvas, rect: &Rect, rrect: &RRect, state: PbarState) {
    // CSS `.pbar-state-error.pbar-err-recede .pbar-track` swaps to a faint
    // ember band so the bare track reads as "errored" once the fill recedes.
    let stops: &[Color4f] = match state {
        PbarState::ErrorRecede => &[
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.15),
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06),
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.15),
        ],
        _ => &[
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
            Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 0.12),
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        ],
    };
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(stops, None),
        Some(&[0.0_f32, 0.5, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        canvas.draw_rrect(*rrect, &p);
    }
    // Inset 1px hairline rim — CSS `box-shadow: inset 0 0 0 1px rgba(229,184,197,0.12)`
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(0.5);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.12),
        None,
    );
    canvas.draw_rrect(*rrect, &rim);
}

fn draw_fill(canvas: &Canvas, rect: &Rect, rrect: &RRect, state: PbarState) {
    let stops: &[Color4f] = match state {
        PbarState::Normal | PbarState::ErrorShimmer | PbarState::ErrorRecede => &[
            Color4f::new(212.0 / 255.0, 168.0 / 255.0, 184.0 / 255.0, 1.0), // #D4A8B8
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0), // #E5B8C5
            Color4f::new(244.0 / 255.0, 212.0 / 255.0, 222.0 / 255.0, 1.0), // #F4D4DE
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0), // #E5B8C5
            Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 1.0), // #C9A5D4
        ],
        PbarState::Complete => &[
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0), // #E5B8C5
            Color4f::new(244.0 / 255.0, 212.0 / 255.0, 222.0 / 255.0, 1.0), // #F4D4DE
            Color4f::new(232.0 / 255.0, 212.0 / 255.0, 168.0 / 255.0, 1.0), // #E8D4A8
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0), // #E5B8C5
        ],
        // CSS `.pbar-state-error.pbar-err-rose .pbar-fill`:
        // linear-gradient(90deg, #A35A6C 0%, #C96A7A 50%, #D4889A 100%)
        PbarState::ErrorRose => &[
            Color4f::new(163.0 / 255.0, 90.0 / 255.0, 108.0 / 255.0, 1.0), // #A35A6C
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 1.0), // #C96A7A
            Color4f::new(212.0 / 255.0, 136.0 / 255.0, 154.0 / 255.0, 1.0), // #D4889A
        ],
    };
    let positions: &[f32] = match state {
        PbarState::Normal | PbarState::ErrorShimmer | PbarState::ErrorRecede => {
            &[0.0, 0.30, 0.50, 0.70, 1.0]
        }
        PbarState::Complete => &[0.0, 0.40, 0.80, 1.0],
        PbarState::ErrorRose => &[0.0, 0.50, 1.0],
    };
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(stops, None),
        Some(positions),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        canvas.draw_rrect(*rrect, &p);
    }
}

fn draw_flow(
    canvas: &Canvas,
    rect: &Rect,
    rrect: &RRect,
    time: f32,
    motion_speed: f32,
    opacity: f32,
    period_seconds: f32,
) {
    // CSS `pbar-flow` — 40%-wide pearl shimmer translating from -40% to 140%
    // over `period_seconds` linear (default 3s; ErrorRose lifts to 8s),
    // clipped to the fill's rrect. `opacity` scales the alpha of the
    // gradient stops (CSS `.pbar-err-rose .pbar-flow { opacity: 0.35 }`).
    let speed = motion_speed.max(0.0001);
    let period = period_seconds / speed;
    let phase = (time / period).rem_euclid(1.0);
    let flow_w = rect.width() * 0.40;
    let pos_left = rect.left + (-0.40 + phase * 1.80) * rect.width();
    let pos_right = pos_left + flow_w;

    let saved = canvas.save();
    canvas.clip_rrect(*rrect, None, Some(true));
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(pos_left, rect.top),
            Point::new(pos_right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.0),
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.35 * opacity),
                Color4f::new(255.0 / 255.0, 255.0 / 255.0, 255.0 / 255.0, 0.55 * opacity),
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.35 * opacity),
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.30, 0.50, 0.70, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        p.set_blend_mode(BlendMode::Screen);
        canvas.draw_rect(*rect, &p);
    }
    canvas.restore_to_count(saved);
}

fn draw_bloom(canvas: &Canvas, rect: &Rect, state: PbarState) {
    // Pearl glow at the leading edge — CSS `.pbar-bloom { right:-6, width:14, blur:3 }`.
    // Error states swap the bloom to ember; Recede skips it (the fill is
    // shrinking and a leading-edge halo would feel celebratory).
    if state == PbarState::ErrorRecede {
        return;
    }
    let bloom_alpha = match state {
        PbarState::Normal | PbarState::ErrorShimmer => 1.0,
        PbarState::Complete => 1.2,
        PbarState::ErrorRose => 0.7,
        PbarState::ErrorRecede => 0.0,
    };
    let ember = state == PbarState::ErrorRose;
    let cx = rect.right - 1.0;
    let cy = (rect.top + rect.bottom) * 0.5;
    let radius = 9.0; // ellipse approx
    let (c0, c1) = if ember {
        (
            Color4f::new(212.0 / 255.0, 136.0 / 255.0, 154.0 / 255.0, 0.9 * bloom_alpha),
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.4 * bloom_alpha),
        )
    } else {
        (
            Color4f::new(255.0 / 255.0, 230.0 / 255.0, 238.0 / 255.0, 0.9 * bloom_alpha),
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.4 * bloom_alpha),
        )
    };
    if let Some(shader) = gradient_shader::radial(
        Point::new(cx, cy),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[c0, c1, Color4f::new(c1.r, c1.g, c1.b, 0.0)],
            None,
        ),
        Some(&[0.0_f32, 0.40, 0.80][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
        p.set_blend_mode(BlendMode::Screen);
        canvas.draw_circle((cx, cy), radius, &p);
    }
}

/// CSS `.pbar-state-error.pbar-err-rose .pbar-fill { box-shadow: 0 0 8px
/// rgba(201, 106, 122, 0.4) }` — soft ember halo around the fill rrect.
fn draw_error_glow(canvas: &Canvas, fill_rrect: &RRect) {
    let mut g = Paint::default();
    g.set_anti_alias(true);
    g.set_style(PaintStyle::Stroke);
    g.set_stroke_width(2.0);
    g.set_color4f(
        Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.4),
        None,
    );
    g.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
    canvas.draw_rrect(*fill_rrect, &g);
}

/// CSS `.pbar-err-shimmer` — 30%-wide ember+pearl flash sweeping across the
/// track over `ERR_SHIM_DURATION` (1.4s) silk-eased, with a fade-in/out
/// opacity envelope (0% → 0, 20% → 1, 100% → 0).
fn draw_error_shimmer(
    canvas: &Canvas,
    track_rect: &Rect,
    track_rrect: &RRect,
    time: f32,
    motion_speed: f32,
) {
    let speed = motion_speed.max(0.0001);
    let period = ERR_SHIM_DURATION / speed;
    let raw = (time / period).rem_euclid(1.0);
    // Silk easing on the position (CSS `cubic-bezier(0.22, 1, 0.36, 1)`).
    let eased = ewo_core::CubicBezier::SILK.eval(raw);
    // Opacity envelope: 0→0, 0.2→1, 1→0. Linear ramp from 0.0..0.2, linear
    // fade from 0.2..1.0.
    let opacity = if raw < 0.2 {
        raw / 0.2
    } else {
        1.0 - (raw - 0.2) / 0.8
    }
    .clamp(0.0, 1.0);

    let track_w = track_rect.width();
    let shim_w = track_w * 0.30;
    let shim_left = track_rect.left + (-0.30 + eased * 1.60) * track_w;
    let shim_right = shim_left + shim_w;

    let saved = canvas.save();
    canvas.clip_rrect(*track_rrect, None, Some(true));
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(shim_left, track_rect.top),
            Point::new(shim_right, track_rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.0),
                Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.5 * opacity),
                Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.6 * opacity),
                Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.5 * opacity),
                Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.48, 0.50, 0.52, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        p.set_blend_mode(BlendMode::Screen);
        canvas.draw_rect(*track_rect, &p);
    }
    canvas.restore_to_count(saved);
}

/// CSS `.pbar-ring` — expanding rose-rim halo at the leading edge of the
/// fill, 1.4s silk easing, diameter 4px → 320px, opacity 0.9 → 0.0,
/// border 2px → 1px.
fn draw_complete_ring(canvas: &Canvas, fill_rect: &Rect, age: f32) {
    if age < 0.0 || age > RING_DURATION {
        return;
    }
    let t = age / RING_DURATION;
    let eased = ewo_core::CubicBezier::SILK.eval(t.clamp(0.0, 1.0));

    let diameter = 4.0 + (RING_MAX_DIAMETER - 4.0) * eased;
    let opacity = 0.9 * (1.0 - eased);
    let border_w = 2.0 - 1.0 * eased;

    let cx = fill_rect.right;
    let cy = (fill_rect.top + fill_rect.bottom) * 0.5;

    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(border_w.max(0.5));
    p.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, opacity),
        None,
    );
    canvas.draw_circle((cx, cy), diameter * 0.5, &p);
}

fn draw_complete_glow(canvas: &Canvas, _rect: &Rect, rrect: &RRect) {
    // CSS complete-state `box-shadow: 0 0 12px rgba(229, 184, 197, 0.45)`.
    let mut g = Paint::default();
    g.set_anti_alias(true);
    g.set_style(PaintStyle::Stroke);
    g.set_stroke_width(2.5);
    g.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.45),
        None,
    );
    g.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, false));
    canvas.draw_rrect(*rrect, &g);
}
