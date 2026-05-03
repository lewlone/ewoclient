//! `meta_pill` — small informational chips used in the prototype's instance
//! head, settings rows, and anywhere a piece of metadata wants to feel like
//! a discrete fact rather than free text.
//!
//! CSS reference (`StyleSheet2`):
//! ```css
//! .meta-pill {
//!   display: inline-flex; align-items: center; gap: 8px;
//!   font-family: 'JetBrains Mono'; font-size: 10px; letter-spacing: 0.18em;
//!   text-transform: uppercase; color: var(--text-mauve);
//!   padding: 6px 10px; border-radius: 8px;
//!   background: rgba(229, 184, 197, 0.04);
//!   box-shadow: inset 0 0 0 1px rgba(229, 184, 197, 0.1);
//! }
//! .meta-pill .dot {
//!   width: 5px; height: 5px; border-radius: 50%;
//!   background: var(--accent-rose);
//!   box-shadow: 0 0 6px rgba(229, 184, 197, 0.9);
//!   animation: meta-pulse 2.4s ease-in-out infinite;
//! }
//! ```
//!
//! Labels render in JetBrains Mono 10 with 0.18em em-tracking. Optional
//! dot indicator pulses opacity 0.55 ↔ 1.0 over 2.4s ease-in-out (CSS
//! `meta-pulse`).

use skia_safe::{
    BlurStyle, Canvas, Color4f, MaskFilter, Paint, PaintStyle, RRect, Rect,
};

use crate::text::{self, FontStore};

const PIPE_FONT_SIZE: f32 = 10.0;
const PIPE_TRACKING_EM: f32 = 0.18;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;
const RADIUS: f32 = 8.0;
const DOT_SIZE: f32 = 5.0;
const DOT_GAP: f32 = 8.0;
const PULSE_PERIOD_S: f32 = 2.4;

const TEXT_MAUVE: Color4f = Color4f {
    r: 0x9A as f32 / 255.0,
    g: 0x80 as f32 / 255.0,
    b: 0x87 as f32 / 255.0,
    a: 1.0,
};
const ACCENT_ROSE: Color4f = Color4f {
    r: 0xE5 as f32 / 255.0,
    g: 0xB8 as f32 / 255.0,
    b: 0xC5 as f32 / 255.0,
    a: 1.0,
};

/// Compute the visual bounds of a meta pill given its label + dot flag.
/// Width = horizontal padding × 2 + (dot + gap if present) + tracked label width.
/// Height is fixed at the CSS-equivalent (~22px).
pub fn meta_pill_size(label: &str, with_dot: bool, fonts: &FontStore) -> (f32, f32) {
    let font = fonts.jetbrains_mono(PIPE_FONT_SIZE);
    let label_w = text::measure_tracked_em(&font, label, PIPE_TRACKING_EM);
    let mut w = 2.0 * PAD_X + label_w;
    if with_dot {
        w += DOT_SIZE + DOT_GAP;
    }
    let h = 2.0 * PAD_Y + PIPE_FONT_SIZE;
    (w, h)
}

/// Draw a single meta pill anchored at `top_left`. Returns the rect drawn.
/// `time` + `motion_speed` drive the optional dot pulse animation.
pub fn draw_meta_pill(
    canvas: &Canvas,
    top_left: (f32, f32),
    label: &str,
    with_dot: bool,
    time: f32,
    motion_speed: f32,
    fonts: &FontStore,
) -> Rect {
    let (w, h) = meta_pill_size(label, with_dot, fonts);
    let rect = Rect::from_xywh(top_left.0, top_left.1, w, h);
    let rrect = RRect::new_rect_xy(rect, RADIUS, RADIUS);

    // Background fill — `rgba(229,184,197,0.04)`.
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.04),
        None,
    );
    canvas.draw_rrect(rrect, &bg);

    // Inset hairline rim — `box-shadow: inset 0 0 0 1px rgba(229,184,197,0.10)`.
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
        None,
    );
    let inset = rect.with_inset((0.5, 0.5));
    canvas.draw_rrect(
        RRect::new_rect_xy(inset, RADIUS - 0.5, RADIUS - 0.5),
        &rim,
    );

    // Layout: pad_x → [dot + gap if present] → label
    let cy = (rect.top + rect.bottom) * 0.5;
    let mut text_x = rect.left + PAD_X;
    if with_dot {
        let dot_cx = text_x + DOT_SIZE * 0.5;
        let dot_cy = cy;
        // Pulse: 0.55 ↔ 1.0 over 2.4s ease-in-out. Smoothstepped triangle wave.
        let speed = motion_speed.max(0.0001);
        let period = PULSE_PERIOD_S / speed;
        let phase = (time / period).rem_euclid(2.0);
        let triangle = if phase < 1.0 { phase } else { 2.0 - phase };
        let smooth = triangle * triangle * (3.0 - 2.0 * triangle);
        let opacity = 0.55 + 0.45 * smooth;

        // Soft halo behind the dot — CSS `box-shadow: 0 0 6px rgba(229,184,197,0.9)`.
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(
            Color4f::new(
                ACCENT_ROSE.r,
                ACCENT_ROSE.g,
                ACCENT_ROSE.b,
                0.9 * opacity,
            ),
            None,
        );
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
        canvas.draw_circle((dot_cx, dot_cy), DOT_SIZE * 0.5 + 1.0, &halo);

        // Solid rose dot.
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            Color4f::new(ACCENT_ROSE.r, ACCENT_ROSE.g, ACCENT_ROSE.b, opacity),
            None,
        );
        canvas.draw_circle((dot_cx, dot_cy), DOT_SIZE * 0.5, &dot);

        text_x += DOT_SIZE + DOT_GAP;
    }

    // Label.
    let font = fonts.jetbrains_mono(PIPE_FONT_SIZE);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(TEXT_MAUVE, None);
    let (_, m) = font.metrics();
    // Center vertically on cap-mid: baseline = cy + cap_height*0.5.
    let baseline = cy + m.cap_height * 0.5;
    text::draw_tracked_em(
        canvas,
        label,
        (text_x, baseline),
        &font,
        &paint,
        PIPE_TRACKING_EM,
    );

    rect
}

/// Draw a row of meta pills with 8px gaps. Returns the bounding rect of the
/// entire row.
pub fn draw_meta_pill_row(
    canvas: &Canvas,
    top_left: (f32, f32),
    items: &[(&str, bool)],
    time: f32,
    motion_speed: f32,
    fonts: &FontStore,
) -> Rect {
    let mut x = top_left.0;
    let y = top_left.1;
    let mut max_h: f32 = 0.0;
    for (label, with_dot) in items.iter() {
        let r = draw_meta_pill(canvas, (x, y), label, *with_dot, time, motion_speed, fonts);
        x = r.right + 8.0;
        max_h = max_h.max(r.height());
    }
    let total_w = (x - top_left.0 - 8.0).max(0.0);
    Rect::from_xywh(top_left.0, top_left.1, total_w, max_h)
}
