//! `scrollbar` — thin rose-tinted thumb on the right edge of any
//! scrollable region. Read-only, decorative; the actual scroll comes
//! from the wheel handler in `main.rs`. Skipped entirely when the
//! content fits.
//!
//! Visual: 3px-wide rose pill, radius `width/2`, inset 4px from the
//! region's right edge. Min thumb height is 24px so it stays grabbable
//! visually even when content greatly overflows.

use skia_safe::{Canvas, Color4f, Paint, RRect, Rect};

const TRACK_WIDTH: f32 = 3.0;
const TRACK_INSET: f32 = 4.0;
const MIN_THUMB_H: f32 = 24.0;

/// Draw a scrollbar thumb inside `region`'s right edge. Right-aligned;
/// `region` is the *visible* rect of the scrollable content (e.g. the
/// glass-panel inner bounds, or the worlds-list rows region).
///
/// Skips when `content_h <= visible_h` (no scroll needed) or when
/// `visible_h <= 0`.
pub fn draw_scrollbar(canvas: &Canvas, region: Rect, scroll: f32, content_h: f32) {
    let visible_h = region.height();
    if visible_h <= 0.0 || content_h <= visible_h + 0.5 {
        return;
    }
    let max_scroll = (content_h - visible_h).max(1.0);
    let frac = (scroll / max_scroll).clamp(0.0, 1.0);

    let track_h = visible_h - 2.0 * TRACK_INSET;
    let thumb_h = ((visible_h / content_h) * track_h).max(MIN_THUMB_H).min(track_h);
    let thumb_top = region.top + TRACK_INSET + frac * (track_h - thumb_h);
    let thumb_rect = Rect::from_xywh(
        region.right - TRACK_WIDTH - TRACK_INSET,
        thumb_top,
        TRACK_WIDTH,
        thumb_h,
    );
    let thumb_rrect = RRect::new_rect_xy(thumb_rect, TRACK_WIDTH * 0.5, TRACK_WIDTH * 0.5);

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.35),
        None,
    );
    canvas.draw_rrect(thumb_rrect, &paint);
}
