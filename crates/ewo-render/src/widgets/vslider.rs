//! `vslider` — the prototype's pearl-on-thread slider.
//!
//! 2px-tall pill track with a gradient fill from `min` to `value`, plus a
//! 14×14 pearl handle that pulses on a 3s firefly cycle. Click anywhere on
//! the track to jump; press-drag to scrub.
//!
//! CSS reference (`StyleSheet1`):
//! ```css
//! .vslider-row    { position: relative; padding: 12px 6px; }
//! .vslider-track  { height: 2px; background: linear-gradient(90deg, rose 0.10, lav 0.15, rose 0.10); }
//! .vslider-fill   { background: linear-gradient(90deg, #D4A8B8, #E5B8C5 50%, #C9A5D4); }
//! .vslider-handle { width: 14px; height: 14px; transform: translate(-50%, -50%); top: 50%; }
//! .vslider-halo   { inset: -10px (34×34 around handle); blur 4px; firefly 3s ease-in-out infinite }
//! .vslider-core   { inset: 3px; radial-gradient(circle at 35% 30%, #FFF0F4, #E5B8C5 50%, #B47491); }
//! @keyframes firefly {
//!   0%, 100% { opacity: 0.65; transform: scale(1); }
//!   50%      { opacity: 1;    transform: scale(1.15); }
//! }
//! ```
//!
//! Trail particles (`.vslider-trail`) and the floating value pill
//! (`.vslider-value`) are deferred to step 16 polish — not load-bearing for
//! visual parity at rest.

use ewo_core::Settings;
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, Color4f, Contains, MaskFilter, Paint, PaintStyle, Point,
    RRect, Rect, TileMode,
};

const TRACK_INSET_X: f32 = 6.0;
const TRACK_HEIGHT: f32 = 2.0;
const HANDLE_SIZE: f32 = 14.0;

#[derive(Debug, Copy, Clone)]
pub struct VsliderState {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    /// Optional snap step. `None` for continuous values.
    pub step: Option<f32>,
    pub hover: bool,
    pub dragging: bool,
}

impl Default for VsliderState {
    fn default() -> Self {
        Self {
            value: 0.5,
            min: 0.0,
            max: 1.0,
            step: None,
            hover: false,
            dragging: false,
        }
    }
}

impl VsliderState {
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self { value, min, max, step: None, hover: false, dragging: false }
    }

    pub fn with_step(mut self, step: f32) -> Self {
        self.step = Some(step);
        self
    }

    pub fn fraction(&self) -> f32 {
        if self.max <= self.min {
            0.0
        } else {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    /// Drive interaction. Call once per frame from input handling.
    /// `mouse_down` is the current LMB state; rising-edge inside bounds
    /// starts a drag, falling edge ends it. While dragging, mouse moves
    /// outside bounds still update the value (clamped to the track).
    /// Returns `true` whenever `value` changed this frame.
    pub fn drive(&mut self, mouse: (f32, f32), bounds: Rect, mouse_down: bool) -> bool {
        let in_bounds = bounds.contains(Point::new(mouse.0, mouse.1));
        self.hover = in_bounds;

        let was_dragging = self.dragging;
        if !was_dragging && mouse_down && in_bounds {
            self.dragging = true;
        }
        if !mouse_down {
            self.dragging = false;
        }

        if !self.dragging {
            return false;
        }

        let track_left = bounds.left + TRACK_INSET_X;
        let track_w = bounds.width() - 2.0 * TRACK_INSET_X;
        if track_w <= 0.0 {
            return false;
        }
        let frac = ((mouse.0 - track_left) / track_w).clamp(0.0, 1.0);
        let raw = self.min + (self.max - self.min) * frac;
        let snapped = match self.step {
            Some(s) if s > 0.0 => ((raw - self.min) / s).round() * s + self.min,
            _ => raw,
        };
        let clamped = snapped.clamp(self.min, self.max);
        let changed = (clamped - self.value).abs() > f32::EPSILON;
        self.value = clamped;
        changed
    }
}

/// Draw a vslider at the given bounds. Bounds height should be at least
/// `HANDLE_SIZE + halo`; ~36px is comfortable.
pub fn draw_vslider(
    canvas: &Canvas,
    bounds: Rect,
    state: &VsliderState,
    time: f32,
    settings: &Settings,
) {
    let track_left = bounds.left + TRACK_INSET_X;
    let track_right = bounds.right - TRACK_INSET_X;
    let track_w = track_right - track_left;
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let track_top = cy - TRACK_HEIGHT * 0.5;
    let track_rect = Rect::from_xywh(track_left, track_top, track_w, TRACK_HEIGHT);
    let track_rrect = RRect::new_rect_xy(track_rect, TRACK_HEIGHT * 0.5, TRACK_HEIGHT * 0.5);

    draw_track(canvas, &track_rect, &track_rrect);

    let frac = state.fraction();
    let handle_x = track_left + frac * track_w;
    if frac > 0.0 {
        let fill_rect = Rect::from_xywh(track_left, track_top, frac * track_w, TRACK_HEIGHT);
        let fill_rrect =
            RRect::new_rect_xy(fill_rect, TRACK_HEIGHT * 0.5, TRACK_HEIGHT * 0.5);
        draw_fill(canvas, &fill_rect, &fill_rrect);
    }

    draw_handle(
        canvas,
        handle_x,
        cy,
        time,
        settings.motion_speed,
        state.dragging,
        state.hover,
    );
}

fn draw_track(canvas: &Canvas, rect: &Rect, rrect: &RRect) {
    // CSS: rose 0.10 → lav 0.15 → rose 0.10
    let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 0.15),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.5, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) else {
        return;
    };
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    canvas.draw_rrect(*rrect, &p);
}

fn draw_fill(canvas: &Canvas, rect: &Rect, rrect: &RRect) {
    // CSS: #D4A8B8 → #E5B8C5 50% → #C9A5D4
    let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(212.0 / 255.0, 168.0 / 255.0, 184.0 / 255.0, 1.0),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 1.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.5, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) else {
        return;
    };
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    canvas.draw_rrect(*rrect, &p);
}

fn draw_handle(
    canvas: &Canvas,
    cx: f32,
    cy: f32,
    time: f32,
    motion_speed: f32,
    dragging: bool,
    hover: bool,
) {
    // Firefly pulse — 3s ease-in-out, opacity 0.65↔1, scale 1↔1.15.
    let speed = motion_speed.max(0.0001);
    let phase = (time / (3.0 / speed)).rem_euclid(1.0);
    let triangle = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
    let smooth = triangle * triangle * (3.0 - 2.0 * triangle);
    // Hover lights the handle up (brighter, larger halo); dragging more so.
    let halo_mult = if dragging { 1.3 } else if hover { 1.18 } else { 1.0 };
    let halo_alpha = ((0.65 + 0.35 * smooth) * halo_mult).min(1.0);
    let halo_scale = (1.0 + 0.15 * smooth) * if dragging || hover { 1.18 } else { 1.0 };
    // Dragging / hover brightens the handle core.
    let drag_boost = if dragging { 1.2 } else if hover { 1.1 } else { 1.0 };

    let handle_r = HANDLE_SIZE * 0.5;
    // Halo — `inset: -10px` of a 14×14 yields 34×34, so radius 17.
    let halo_r = 17.0 * halo_scale;
    if let Some(halo_shader) = gradient_shader::radial(
        Point::new(cx, cy),
        halo_r,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(255.0 / 255.0, 230.0 / 255.0, 238.0 / 255.0, 0.55 * drag_boost),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20 * drag_boost),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.40, 0.75][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_shader(halo_shader);
        halo.set_alpha_f(halo_alpha);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
        canvas.draw_circle((cx, cy), halo_r, &halo);
    }

    // Core — 8×8 (3px inset of 14×14), radial pearl gradient.
    let core_r = (HANDLE_SIZE - 6.0) * 0.5;
    if let Some(core_shader) = gradient_shader::radial(
        Point::new(cx - core_r * 0.3, cy - core_r * 0.4),
        core_r * 1.6,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 1.0),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, 1.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.50, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        // Outer rose glow (CSS box-shadow: 0 0 6 rgba(229,184,197,0.9))
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.9),
            None,
        );
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
        canvas.draw_circle((cx, cy), core_r + 1.0, &glow);

        let mut core = Paint::default();
        core.set_anti_alias(true);
        core.set_shader(core_shader);
        canvas.draw_circle((cx, cy), core_r, &core);

        // Inset white highlight (CSS inset 0 0 0 0.5px white 0.4).
        let mut highlight = Paint::default();
        highlight.set_anti_alias(true);
        highlight.set_style(PaintStyle::Stroke);
        highlight.set_stroke_width(0.5);
        highlight.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.40), None);
        canvas.draw_circle((cx, cy), core_r - 0.25, &highlight);
    }
    // The full handle radius + 1 gives a thin transparent ring beyond the
    // core that lets the fill peek through visually like the CSS handle does.
    let _ = handle_r;
}
