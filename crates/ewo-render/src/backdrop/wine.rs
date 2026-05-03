//! Wine radial gradient — the static base of the backdrop.
//!
//! CSS:
//! ```css
//! .backdrop {
//!   background:
//!     radial-gradient(ellipse at 35% 40%, var(--bg-wine-b) 0%, var(--bg-wine-a) 45%, #000 80%);
//! }
//! ```
//! `--bg-wine-a = #0A0006`, `--bg-wine-b = #120010`.
//!
//! Default ellipse sizing for radial-gradient is `farthest-corner`, which means
//! the gradient's `100%` stop touches the corner farthest from the center.

use ewo_core::Theme;
use skia_safe::{
    gradient_shader, Canvas, Color4f, Matrix, Paint, Point, Rect, TileMode,
};

pub fn draw(canvas: &Canvas, w: f32, h: f32, theme: &Theme) {
    let cx = 0.35 * w;
    let cy = 0.40 * h;

    // Farthest-corner radii in X and Y.
    let rx = (cx).max(w - cx);
    let ry = (cy).max(h - cy);

    // Skia radial_gradient is a circle; use rx as the base radius and a local
    // matrix to scale Y into an ellipse.
    let radius = rx;
    let mut local = Matrix::default();
    local.pre_translate((cx, cy));
    local.pre_scale((1.0, ry / rx), None);
    local.pre_translate((-cx, -cy));

    let wine_a = color4f_from_srgb(theme.bg_wine_a.to_f32());
    let wine_b = color4f_from_srgb(theme.bg_wine_b.to_f32());
    let black = Color4f::new(0.0, 0.0, 0.0, 1.0);

    let shader = gradient_shader::radial(
        Point::new(cx, cy),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[wine_b, wine_a, black],
            None,
        ),
        Some(&[0.0_f32, 0.45, 0.80][..]),
        TileMode::Clamp,
        None,
        Some(&local),
    )
    .expect("wine radial shader");

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &paint);
}

#[inline]
fn color4f_from_srgb([r, g, b]: [f32; 3]) -> Color4f {
    Color4f::new(r, g, b, 1.0)
}
