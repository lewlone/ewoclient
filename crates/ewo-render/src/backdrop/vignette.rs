//! Vignette — corners darken into the page.
//!
//! CSS:
//! ```css
//! .vf-vignette {
//!   position: absolute; inset: 0;
//!   background:
//!     radial-gradient(ellipse at 50% 60%, transparent 30%, rgba(0,0,0,0.65) 85%);
//! }
//! ```
//!
//! Drawn last in the backdrop stack — over the wine, caustics, and bokeh —
//! so it dims those layers gracefully toward the edges of the card.

use skia_safe::{
    gradient_shader, Canvas, Color4f, Matrix, Paint, Point, Rect, TileMode,
};

pub fn draw(canvas: &Canvas, w: f32, h: f32) {
    let cx = 0.50 * w;
    let cy = 0.60 * h;

    // Farthest-corner radii; ellipse via local matrix.
    let rx = cx.max(w - cx);
    let ry = cy.max(h - cy);
    let radius = rx;

    let mut local = Matrix::default();
    local.pre_translate((cx, cy));
    local.pre_scale((1.0, ry / rx), None);
    local.pre_translate((-cx, -cy));

    // Reduced from CSS's 0.65 → 0.50 to leave more luminance in the corners.
    // OLED-friendly aesthetic: stay dark, but don't crush the layers underneath.
    let inner = Color4f::new(0.0, 0.0, 0.0, 0.0);
    let outer = Color4f::new(0.0, 0.0, 0.0, 0.50);

    let shader = gradient_shader::radial(
        Point::new(cx, cy),
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(&[inner, outer], None),
        Some(&[0.30_f32, 0.85][..]),
        TileMode::Clamp,
        None,
        Some(&local),
    )
    .expect("vignette shader");

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_shader(shader);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &paint);
}
