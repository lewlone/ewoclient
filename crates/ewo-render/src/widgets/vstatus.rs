//! `vstatus` — single-line status text with a cross-fade transition.
//!
//! When the value changes, the old string slides up + blurs out while the
//! new string slides up + blurs in. Both halves last 500ms with silk
//! easing, run concurrently.
//!
//! CSS reference (`StyleSheet1` `.vstatus`, `.vstatus-in`, `.vstatus-out`,
//! `@keyframes status-in`, `@keyframes status-out`):
//! ```css
//! @keyframes status-in {
//!   from { opacity: 0; transform: translateY(8px);  filter: blur(3px); }
//!   to   { opacity: 1; transform: translateY(0);    filter: blur(0); }
//! }
//! @keyframes status-out {
//!   from { opacity: 1; transform: translateY(0);   }
//!   to   { opacity: 0; transform: translateY(-10px); filter: blur(3px); }
//! }
//! ```
//!
//! The widget is text-only — caller supplies the font + paint. Used by the
//! Launching screen's stage label (and reusable elsewhere).

use ewo_core::CubicBezier;
use skia_safe::{
    canvas::SaveLayerRec, image_filters, Canvas, Font, Paint, TileMode,
};

/// Crossfade duration in seconds.
const FADE_SECONDS: f32 = 0.5;

#[derive(Debug, Clone, Default)]
pub struct VstatusState {
    pub current: String,
    pub fading_out: Option<String>,
    /// 0..1, 1 = transition complete. Drives both the in and out halves.
    pub anim: f32,
}

impl VstatusState {
    pub fn new(initial: &str) -> Self {
        Self {
            current: initial.to_string(),
            fading_out: None,
            anim: 1.0,
        }
    }

    /// Set the new value. If it matches the current, no-op. Otherwise
    /// snapshots the current as the fading-out copy and resets `anim` to 0.
    pub fn set(&mut self, text: &str) {
        if self.current != text {
            self.fading_out = Some(std::mem::take(&mut self.current));
            self.current = text.to_string();
            self.anim = 0.0;
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.anim < 1.0 {
            self.anim = (self.anim + dt / FADE_SECONDS).min(1.0);
            if self.anim >= 1.0 {
                self.fading_out = None;
            }
        }
    }
}

/// Draw the status text with a cross-fade animation. `origin` is
/// `(left, baseline)` — the same convention as `Canvas::draw_str`.
pub fn draw_vstatus(
    canvas: &Canvas,
    origin: (f32, f32),
    font: &Font,
    paint: &Paint,
    state: &VstatusState,
) {
    let eased = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    // Outgoing — opacity 1→0, translateY(0 → -10), blur(0 → 3px).
    if let Some(prev) = state.fading_out.as_deref() {
        let alpha = 1.0 - eased;
        let ty = -10.0 * eased;
        let blur = 3.0 * eased;
        draw_with_effects(canvas, prev, origin, font, paint, alpha, ty, blur);
    }

    // Incoming — opacity 0→1, translateY(8 → 0), blur(3 → 0).
    let alpha = eased;
    let ty = 8.0 * (1.0 - eased);
    let blur = 3.0 * (1.0 - eased);
    draw_with_effects(canvas, &state.current, origin, font, paint, alpha, ty, blur);
}

fn draw_with_effects(
    canvas: &Canvas,
    text: &str,
    origin: (f32, f32),
    font: &Font,
    paint: &Paint,
    alpha: f32,
    translate_y: f32,
    blur_sigma: f32,
) {
    if alpha <= 0.001 || text.is_empty() {
        return;
    }
    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(alpha);
    if blur_sigma > 0.05 {
        if let Some(filter) = image_filters::blur(
            (blur_sigma, blur_sigma),
            TileMode::Decal,
            None,
            None,
        ) {
            layer_paint.set_image_filter(filter);
        }
    }
    let saved = canvas.save();
    let rec = SaveLayerRec::default().paint(&layer_paint);
    canvas.save_layer(&rec);
    canvas.translate((0.0, translate_y));
    canvas.draw_str(text, origin, font, paint);
    canvas.restore();
    canvas.restore_to_count(saved);
}
