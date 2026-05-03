//! `vtoggle` — the prototype's pill toggle (`.settings-toggle`).
//!
//! 44×22 rounded pill with a 16×16 pearl that slides between left:2 and
//! right:-2. Off and on states crossfade colors and the on state adds a
//! soft outer glow plus a brighter pearl halo.
//!
//! CSS reference (`StyleSheet2`):
//! ```css
//! .settings-toggle {
//!   width: 44px; height: 22px; border-radius: 999px;
//!   background: rgba(229, 184, 197, 0.08);
//!   border: 1px solid rgba(229, 184, 197, 0.18);
//!   transition: background 320ms, border-color 320ms, box-shadow 320ms;
//! }
//! .settings-toggle.on {
//!   background: rgba(229, 184, 197, 0.22);
//!   border-color: rgba(229, 184, 197, 0.45);
//!   box-shadow: 0 0 14px rgba(229, 184, 197, 0.28);
//! }
//! .settings-toggle-pearl {
//!   width: 16px; height: 16px;
//!   background: radial-gradient(circle at 30% 30%, #FFF6F0, #E5B8C5 60%, #C8A4B4);
//!   transition: left 320ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 320ms;
//!   box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3);
//! }
//! .settings-toggle.on .settings-toggle-pearl {
//!   left: calc(100% - 18px);
//!   box-shadow: 0 0 10px rgba(255, 246, 240, 0.7), 0 1px 4px rgba(0, 0, 0, 0.3);
//! }
//! ```

use ewo_core::CubicBezier;
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, Color4f, Contains, MaskFilter, Paint, PaintStyle, Point,
    RRect, Rect, TileMode,
};

pub const TOGGLE_W: f32 = 44.0;
pub const TOGGLE_H: f32 = 22.0;

const PEARL_SIZE: f32 = 16.0;
const PEARL_PAD: f32 = 2.0; // CSS `left: 2` resting position

#[derive(Default, Debug, Copy, Clone)]
pub struct VtoggleState {
    pub on: bool,
    pub hover: bool,
    /// 0..1, tracks `on` over the CSS 320ms silk transition. Drives both
    /// pearl position and color crossfades.
    pub anim: f32,
}

impl VtoggleState {
    pub const fn new(on: bool) -> Self {
        Self { on, hover: false, anim: if on { 1.0 } else { 0.0 } }
    }

    /// Update hover state and react to a click. Returns `true` when the
    /// toggle flipped on this frame. `clicked` should be set on the
    /// mouse-up frame (after a press inside the bounds).
    pub fn handle(&mut self, mouse: (f32, f32), bounds: Rect, clicked: bool) -> bool {
        let in_bounds = bounds.contains(Point::new(mouse.0, mouse.1));
        self.hover = in_bounds;
        if clicked && in_bounds {
            self.on = !self.on;
            return true;
        }
        false
    }

    /// Per-frame animation tick — 320ms approach to the target.
    pub fn tick(&mut self, dt: f32) {
        let target = if self.on { 1.0 } else { 0.0 };
        let max_step = dt / 0.32;
        let delta: f32 = target - self.anim;
        self.anim += delta.clamp(-max_step, max_step);
    }
}

/// Draw a vtoggle at the given bounds. The bounds' size should be roughly
/// `(TOGGLE_W, TOGGLE_H)` — passing larger bounds will stretch the pill.
pub fn draw_vtoggle(canvas: &Canvas, bounds: Rect, state: &VtoggleState) {
    let radius = bounds.height() * 0.5;
    let rrect = RRect::new_rect_xy(bounds, radius, radius);

    // Silk-eased animation for both color crossfade and pearl slide.
    let anim = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    draw_outer_glow(canvas, &rrect, anim);
    draw_pill_bg(canvas, &rrect, anim);
    draw_pill_border(canvas, &rrect, anim);
    draw_pearl(canvas, &bounds, anim);
}

fn draw_outer_glow(canvas: &Canvas, rrect: &RRect, anim: f32) {
    if anim < 0.001 {
        return;
    }
    // CSS `box-shadow: 0 0 14px rgba(229, 184, 197, 0.28)` — sigma 7, alpha
    // ramped by anim so the off→on transition fades in the glow.
    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_style(PaintStyle::Stroke);
    glow.set_stroke_width(6.0);
    glow.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.28 * anim),
        None,
    );
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 7.0, false));
    canvas.draw_rrect(*rrect, &glow);
}

fn draw_pill_bg(canvas: &Canvas, rrect: &RRect, anim: f32) {
    // Off: rgba(229,184,197,0.08); On: rgba(229,184,197,0.22).
    let alpha = 0.08 + (0.22 - 0.08) * anim;
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, alpha),
        None,
    );
    canvas.draw_rrect(*rrect, &bg);
}

fn draw_pill_border(canvas: &Canvas, rrect: &RRect, anim: f32) {
    // Off: 0.18; On: 0.45.
    let alpha = 0.18 + (0.45 - 0.18) * anim;
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, alpha),
        None,
    );
    canvas.draw_rrect(*rrect, &border);
}

fn draw_pearl(canvas: &Canvas, bounds: &Rect, anim: f32) {
    let off_x = bounds.left + PEARL_PAD;
    let on_x = bounds.right - PEARL_PAD - PEARL_SIZE;
    let pearl_left = off_x + (on_x - off_x) * anim;
    let pearl_cx = pearl_left + PEARL_SIZE * 0.5;
    let pearl_cy = (bounds.top + bounds.bottom) * 0.5;
    let pearl_r = PEARL_SIZE * 0.5;

    // Drop shadow `0 1px 4px rgba(0,0,0,0.3)`.
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.3), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_circle((pearl_cx, pearl_cy + 1.0), pearl_r, &shadow);

    // On-state outer halo `0 0 10px rgba(255,246,240,0.7)`.
    if anim > 0.001 {
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(
            Color4f::new(255.0 / 255.0, 246.0 / 255.0, 240.0 / 255.0, 0.7 * anim),
            None,
        );
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
        canvas.draw_circle((pearl_cx, pearl_cy), pearl_r, &halo);
    }

    // Pearl body — `radial-gradient(circle at 30% 30%, #FFF6F0, #E5B8C5 60%, #C8A4B4)`.
    let grad_origin = Point::new(pearl_cx - pearl_r * 0.4, pearl_cy - pearl_r * 0.4);
    if let Some(shader) = gradient_shader::radial(
        grad_origin,
        pearl_r * 1.6,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(255.0 / 255.0, 246.0 / 255.0, 240.0 / 255.0, 1.0),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                Color4f::new(200.0 / 255.0, 164.0 / 255.0, 180.0 / 255.0, 1.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.60, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut pearl = Paint::default();
        pearl.set_anti_alias(true);
        pearl.set_shader(shader);
        canvas.draw_circle((pearl_cx, pearl_cy), pearl_r, &pearl);
    }

    // Inset white highlight ring.
    let mut highlight = Paint::default();
    highlight.set_anti_alias(true);
    highlight.set_style(PaintStyle::Stroke);
    highlight.set_stroke_width(0.5);
    highlight.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.40), None);
    canvas.draw_circle((pearl_cx, pearl_cy), pearl_r - 0.4, &highlight);
}
