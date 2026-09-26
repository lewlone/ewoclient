//! Window-chrome hit testing (resize border, caption strip, buttons) and the card-local
//! coordinate helpers the widget hit-tests run on. Moved from `main.rs` unchanged.

use ewo_render::{app_window, skia_safe};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::ResizeDirection;

pub(crate) const RESIZE_BORDER_LP: f64 = 8.0;
pub(crate) const CAPTION_HEIGHT_LP: f64 = 32.0;

// Card inset (logical px). Mirrors `app_window::CARD_INSET`. Used to convert
// cursor positions from window-local to card-local for widget hit-testing.
// 0 — the card fills the window (see app_window::CARD_INSET).
pub(crate) const CARD_INSET_LP: f64 = 0.0;

#[derive(Copy, Clone, Debug)]
pub(crate) enum Zone {
    Caption,
    Resize(ResizeDirection),
}

pub(crate) fn hit_test(
    pos: PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
    scale: f64,
) -> Option<Zone> {
    let border = RESIZE_BORDER_LP * scale;
    let caption = CAPTION_HEIGHT_LP * scale;
    let (x, y) = (pos.x, pos.y);
    let (w, h) = (size.width as f64, size.height as f64);

    let on_top = y >= 0.0 && y < border;
    let on_bottom = y > h - border;
    let on_left = x >= 0.0 && x < border;
    let on_right = x > w - border;

    use ResizeDirection::*;
    let dir = match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => Some(NorthWest),
        (true, _, _, true) => Some(NorthEast),
        (_, true, true, _) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, _, _, _) => Some(North),
        (_, true, _, _) => Some(South),
        (_, _, true, _) => Some(West),
        (_, _, _, true) => Some(East),
        _ => None,
    };
    if let Some(d) = dir {
        return Some(Zone::Resize(d));
    }
    // Top-right minimize / close buttons sit in the caption strip but must be
    // clickable, not a drag handle — exclude them before the caption check.
    let lx = (x / scale) as f32;
    let ly = (y / scale) as f32;
    let (min_btn, close_btn) = app_window::window_button_bounds((w / scale) as f32);
    if rect_contains(&min_btn, (lx, ly)) || rect_contains(&close_btn, (lx, ly)) {
        return None;
    }
    if y >= 0.0 && y < caption {
        return Some(Zone::Caption);
    }
    None
}

/// Convert a window-local cursor position (physical px) into card-local
/// (logical px), matching the coord space widget code uses.
pub(crate) fn cursor_card_local(cursor: PhysicalPosition<f64>, scale: f64) -> (f32, f32) {
    let lp_x = cursor.x / scale;
    let lp_y = cursor.y / scale;
    ((lp_x - CARD_INSET_LP) as f32, (lp_y - CARD_INSET_LP) as f32)
}

/// Card content width in card-local logical pixels (window minus 2× card inset).
pub(crate) fn card_content_width(size: PhysicalSize<u32>, scale: f64) -> f32 {
    let logical_w = size.width as f64 / scale;
    (logical_w - 2.0 * CARD_INSET_LP) as f32
}

/// Card content height in card-local logical pixels.
pub(crate) fn card_content_height(size: PhysicalSize<u32>, scale: f64) -> f32 {
    let logical_h = size.height as f64 / scale;
    (logical_h - 2.0 * CARD_INSET_LP) as f32
}

pub(crate) fn rect_contains(rect: &skia_safe::Rect, p: (f32, f32)) -> bool {
    p.0 >= rect.left && p.0 <= rect.right && p.1 >= rect.top && p.1 <= rect.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> PhysicalSize<u32> { PhysicalSize::new(1000, 600) }
    fn zone(x: f64, y: f64, s: f64) -> Option<Zone> { hit_test(PhysicalPosition::new(x, y), size(), s) }
    fn centre(r: &skia_safe::Rect, s: f64) -> (f64, f64) {
        (((r.left + r.right) * 0.5) as f64 * s, ((r.top + r.bottom) * 0.5) as f64 * s)
    }

    #[test]
    fn resize_zones_win_at_the_corners_and_edges() {
        use ResizeDirection::*;
        for s in [1.0, 2.0] {
            for (x, y, d) in [
                (1.0, 1.0, NorthWest), (999.0, 1.0, NorthEast), (1.0, 599.0, SouthWest),
                (999.0, 599.0, SouthEast), (500.0, 2.0, North), (500.0, 598.0, South),
                (2.0, 300.0, West), (998.0, 300.0, East),
            ] {
                assert!(matches!(zone(x, y, s), Some(Zone::Resize(dir)) if dir == d), "at ({x},{y}) s={s}");
            }
        }
    }

    /// Everything off the resize border: the caption strip drags, the buttons
    /// are clickable, the middle of the window is plain content.
    #[test]
    fn caption_buttons_and_centre_are_not_resize() {
        for s in [1.0, 2.0] {
            // 24px down sits inside the 32px / 64px caption, clear of the border.
            let got = zone(200.0, 24.0, s);
            assert!(matches!(got, Some(Zone::Caption)), "{got:?} s={s}");
            // Middle of the window: content, not chrome.
            assert!(zone(500.0, 300.0, s).is_none(), "centre s={s}");
            let (min, close) = app_window::window_button_bounds((size().width as f64 / s) as f32);
            for (cx, cy) in [centre(&min, s), centre(&close, s)] {
                assert!(zone(cx, cy, s).is_none(), "button at ({cx},{cy}) s={s}");
            }
        }
    }

    #[test]
    fn rect_contains_is_inclusive_on_all_four_edges() {
        let r = skia_safe::Rect::from_ltrb(10.0, 20.0, 30.0, 40.0);
        for corner in [(10.0, 20.0), (30.0, 20.0), (10.0, 40.0), (30.0, 40.0)] {
            assert!(rect_contains(&r, corner), "corner {corner:?}");
        }
        for outside in [(9.0, 30.0), (31.0, 30.0), (20.0, 19.0), (20.0, 41.0)] {
            assert!(!rect_contains(&r, outside), "outside {outside:?}");
        }
    }

    #[test]
    fn card_helpers_divide_by_scale() {
        let p = PhysicalPosition::new(200.0, 100.0);
        assert_eq!((cursor_card_local(p, 1.0), cursor_card_local(p, 2.0)), ((200.0, 100.0), (100.0, 50.0)));
        assert_eq!((card_content_width(size(), 1.0), card_content_width(size(), 2.0)), (1000.0, 500.0));
        assert_eq!((card_content_height(size(), 1.0), card_content_height(size(), 2.0)), (600.0, 300.0));
    }
}
