//! Top tab bar — `.state-picker` from the prototype, repurposed as the
//! launcher's screen navigator. Drawn on every screen.
//!
//! CSS:
//! ```css
//! .state-picker {
//!   position: absolute; top: 20px; left: 50%; transform: translateX(-50%);
//!   display: flex; gap: 4px; padding: 4px;
//!   border-radius: 14px;
//!   background: rgba(10, 0, 6, 0.5);
//!   backdrop-filter: blur(12px);
//!   box-shadow: inset 0 0 0 1px rgba(229, 184, 197, 0.18);
//! }
//! .state-picker button {
//!   border: 0; background: transparent;
//!   color: var(--text-mauve);
//!   font: 10px JetBrains Mono;
//!   letter-spacing: 0.18em;
//!   text-transform: uppercase;
//!   padding: 8px 12px;
//!   border-radius: 10px;
//! }
//! .state-picker button.active {
//!   color: var(--text-pearl);
//!   background: rgba(229, 184, 197, 0.12);
//!   box-shadow: inset 0 0 0 1px rgba(229, 184, 197, 0.3);
//! }
//! ```

use ewo_core::Screen;
use skia_safe::{Canvas, Color, Color4f, Paint, PaintStyle, RRect, Rect};

use crate::text::{self, FontStore};

const BAR_TOP: f32 = 20.0;
const BAR_PAD: f32 = 4.0;
const BAR_RADIUS: f32 = 14.0;
const ITEM_PAD_X: f32 = 12.0;
const ITEM_PAD_Y: f32 = 8.0;
const ITEM_RADIUS: f32 = 10.0;
const ITEM_GAP: f32 = 4.0;
const LABEL_SIZE: f32 = 10.0;
const LABEL_TRACKING_EM: f32 = 0.18;

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);

/// Compute hit-rects for each tab in card-local coords. Caller (main.rs)
/// uses these to route mouse clicks to a `Screen` change.
pub fn tab_bounds(card_w: f32, fonts: &FontStore) -> Vec<(Screen, Rect)> {
    let font = fonts.jetbrains_mono(LABEL_SIZE);
    let labels: Vec<&str> = Screen::all().iter().map(|s| s.tab_label()).collect();
    let item_widths: Vec<f32> = labels
        .iter()
        .map(|l| text::measure_tracked_em(&font, l, LABEL_TRACKING_EM))
        .collect();
    let item_outer_widths: Vec<f32> = item_widths.iter().map(|w| w + 2.0 * ITEM_PAD_X).collect();
    let inner_width: f32 =
        item_outer_widths.iter().sum::<f32>() + ITEM_GAP * (labels.len() as f32 - 1.0);
    let item_height = LABEL_SIZE + 2.0 * ITEM_PAD_Y;
    let bar_outer_w = inner_width + 2.0 * BAR_PAD;
    let bar_left = (card_w - bar_outer_w) * 0.5;

    let mut out = Vec::with_capacity(Screen::all().len());
    let mut x = bar_left + BAR_PAD;
    for (i, screen) in Screen::all().iter().enumerate() {
        let outer = item_outer_widths[i];
        let rect = Rect::from_xywh(x, BAR_TOP + BAR_PAD, outer, item_height);
        out.push((*screen, rect));
        x += outer + ITEM_GAP;
    }
    out
}

/// Draw the tab bar with `current` highlighted as active. `hovered` (if any)
/// gets the hero-style glow + pearl text so the cursor lights tabs up.
pub fn draw_tab_bar(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    current: Screen,
    hovered: Option<Screen>,
) {
    let bounds = tab_bounds(card_w, fonts);
    if bounds.is_empty() {
        return;
    }

    // Compute outer bar bounds from the union of item bounds + padding.
    let first = bounds[0].1;
    let last = bounds[bounds.len() - 1].1;
    let bar_rect = Rect::from_ltrb(
        first.left - BAR_PAD,
        first.top - BAR_PAD,
        last.right + BAR_PAD,
        first.bottom + BAR_PAD,
    );
    let bar_rrect = RRect::new_rect_xy(bar_rect, BAR_RADIUS, BAR_RADIUS);

    // Bar background pill.
    let mut bar_bg = Paint::default();
    bar_bg.set_anti_alias(true);
    bar_bg.set_color4f(Color4f::new(10.0 / 255.0, 0.0, 6.0 / 255.0, 0.5), None);
    canvas.draw_rrect(bar_rrect, &bar_bg);

    let mut bar_rim = Paint::default();
    bar_rim.set_anti_alias(true);
    bar_rim.set_style(PaintStyle::Stroke);
    bar_rim.set_stroke_width(1.0);
    bar_rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.18),
        None,
    );
    canvas.draw_rrect(bar_rrect, &bar_rim);

    let font = fonts.jetbrains_mono(LABEL_SIZE);
    let (_, metrics) = font.metrics();

    // Items
    for (screen, rect) in &bounds {
        let active = *screen == current;
        let is_hover = !active && hovered == Some(*screen);

        if active {
            let pill = RRect::new_rect_xy(*rect, ITEM_RADIUS, ITEM_RADIUS);
            let mut bg = Paint::default();
            bg.set_anti_alias(true);
            bg.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.12),
                None,
            );
            canvas.draw_rrect(pill, &bg);

            let mut stroke = Paint::default();
            stroke.set_anti_alias(true);
            stroke.set_style(PaintStyle::Stroke);
            stroke.set_stroke_width(1.0);
            stroke.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.3),
                None,
            );
            canvas.draw_rrect(pill, &stroke);
        }

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(if active || is_hover { TEXT_PEARL } else { TEXT_MAUVE });

        let baseline = rect.top + ITEM_PAD_Y + (-metrics.ascent);
        let origin = (rect.left + ITEM_PAD_X, baseline);
        if is_hover {
            text::draw_tracked_glow(
                canvas,
                screen.tab_label(),
                origin,
                &font,
                LABEL_TRACKING_EM,
                0.85,
            );
        }
        text::draw_tracked_em(
            canvas,
            screen.tab_label(),
            origin,
            &font,
            &paint,
            LABEL_TRACKING_EM,
        );
    }
}
