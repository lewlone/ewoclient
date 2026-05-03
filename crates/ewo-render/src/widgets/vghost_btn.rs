//! `vghost_btn` — the prototype's two ghost-button variants.
//!
//! A "ghost" button is a transparent rect with a 1px border that brightens
//! on hover, plus a soft glow. The prototype has two flavors:
//!   - **Pearl** (`.settings-pathbtn`) — rose hairline border, italic 13,
//!     8px corner radius. Used by the Paths tab's Browse buttons.
//!   - **Danger** (`.settings-danger`) — ember hairline border, italic 14,
//!     999px pill radius. Used for "Reset preferences" in Advanced.
//!
//! These don't share enough chrome with `vbtn` to justify reusing it (no
//! tint, no rim cycle, no sheen sweep) — they live in their own module.
//!
//! CSS reference (`StyleSheet2` `.settings-pathbtn` and `.settings-danger`).

use ewo_core::CubicBezier;
use skia_safe::{
    BlurStyle, Canvas, Color4f, Contains, MaskFilter, Paint, PaintStyle, Point, RRect, Rect,
};

use crate::text::FontStore;

#[derive(Copy, Clone, Debug)]
pub enum GhostKind {
    Pearl,
    Danger,
}

#[derive(Default, Debug, Copy, Clone)]
pub struct VghostBtnState {
    pub hover: bool,
    /// 0..1, smoothly tracks `hover` over the CSS 240ms transition.
    pub hover_anim: f32,
}

impl VghostBtnState {
    /// Update hover from cursor + return `true` when clicked (mouse-just-pressed inside).
    pub fn handle(&mut self, mouse: (f32, f32), bounds: Rect, mouse_just_pressed: bool) -> bool {
        let in_bounds = bounds.contains(Point::new(mouse.0, mouse.1));
        self.hover = in_bounds;
        mouse_just_pressed && in_bounds
    }

    pub fn tick(&mut self, dt: f32) {
        let target = if self.hover { 1.0 } else { 0.0 };
        let max_step = dt / 0.24;
        let delta: f32 = target - self.hover_anim;
        self.hover_anim += delta.clamp(-max_step, max_step);
    }
}

pub fn draw_vghost_btn(
    canvas: &Canvas,
    bounds: Rect,
    label: &str,
    state: &VghostBtnState,
    kind: GhostKind,
    fonts: &FontStore,
) {
    let radius = match kind {
        GhostKind::Pearl => 8.0,
        GhostKind::Danger => bounds.height() * 0.5,
    };
    let rrect = RRect::new_rect_xy(bounds, radius, radius);
    let anim = CubicBezier::SILK.eval(state.hover_anim.clamp(0.0, 1.0));

    if anim > 0.001 {
        draw_glow(canvas, &rrect, kind, anim);
    }

    let (border_off, border_on, label_off, label_on, font_size) = match kind {
        GhostKind::Pearl => (
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.18),
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
            Color4f::new(196.0 / 255.0, 175.0 / 255.0, 181.0 / 255.0, 1.0),
            Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 1.0),
            13.0,
        ),
        GhostKind::Danger => (
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.30),
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.70),
            Color4f::new(212.0 / 255.0, 136.0 / 255.0, 154.0 / 255.0, 1.0),
            Color4f::new(229.0 / 255.0, 176.0 / 255.0, 189.0 / 255.0, 1.0),
            14.0,
        ),
    };

    // Border
    let border = lerp_color(border_off, border_on, anim);
    let mut bp = Paint::default();
    bp.set_anti_alias(true);
    bp.set_style(PaintStyle::Stroke);
    bp.set_stroke_width(1.0);
    bp.set_color4f(border, None);
    canvas.draw_rrect(rrect, &bp);

    // Label — italic Newsreader at the variant's font size.
    let font = fonts.newsreader(font_size);
    let label_color = lerp_color(label_off, label_on, anim);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(label_color, None);
    let (advance, _) = font.measure_str(label, Some(&tp));
    let (_, m) = font.metrics();
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    canvas.draw_str(label, (cx - advance * 0.5, baseline), &font, &tp);
}

fn draw_glow(canvas: &Canvas, rrect: &RRect, kind: GhostKind, anim: f32) {
    let (color, sigma) = match kind {
        GhostKind::Pearl => (
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20 * anim),
            7.0,
        ),
        GhostKind::Danger => (
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.20 * anim),
            8.0,
        ),
    };
    let mut g = Paint::default();
    g.set_anti_alias(true);
    g.set_style(PaintStyle::Stroke);
    g.set_stroke_width(4.0);
    g.set_color4f(color, None);
    g.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, sigma, false));
    canvas.draw_rrect(*rrect, &g);
}

fn lerp_color(a: Color4f, b: Color4f, t: f32) -> Color4f {
    Color4f::new(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}
