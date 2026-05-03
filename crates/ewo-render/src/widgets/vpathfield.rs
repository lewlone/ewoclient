//! `vpathfield` — read-only path input + Browse button row.
//!
//! Renders the prototype's `.settings-pathfield` (grid: 1fr input + auto
//! Browse button, gap 12). The "input" is a faux text field — text input
//! is out of v1 scope (no IME, no caret), so the value renders as a
//! styled label inside the same chrome the prototype uses for the input.
//! The Browse button is a `vghost_btn`.
//!
//! CSS reference (`StyleSheet2` `.settings-pathfield`, `.settings-pathinput`,
//! `.settings-pathbtn`).

use ewo_core::CubicBezier;
use skia_safe::{
    BlurStyle, Canvas, Color, Color4f, Contains, MaskFilter, Paint, PaintStyle, Point, RRect,
    Rect,
};

use crate::text::FontStore;
use crate::widgets::vghost_btn::{draw_vghost_btn, GhostKind, VghostBtnState};

const FIELD_RADIUS: f32 = 8.0;
const FIELD_PAD_X: f32 = 14.0;
const ROW_GAP: f32 = 12.0;
const BROWSE_W: f32 = 88.0;
const BROWSE_H: f32 = 38.0;
const BROWSE_LABEL: &str = "Browse…";

#[derive(Debug, Clone, Default)]
pub struct VpathfieldState {
    pub value: String,
    pub hover_input: bool,
    /// 0..1, smoothly tracks `hover_input` for the focus-style border lift.
    pub hover_anim: f32,
    pub browse: VghostBtnState,
}

impl VpathfieldState {
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            value: initial.into(),
            hover_input: false,
            hover_anim: 0.0,
            browse: VghostBtnState::default(),
        }
    }

    pub fn tick(&mut self, dt: f32) {
        let target = if self.hover_input { 1.0 } else { 0.0 };
        let max_step = dt / 0.24;
        let delta: f32 = target - self.hover_anim;
        self.hover_anim += delta.clamp(-max_step, max_step);
        self.browse.tick(dt);
    }

    /// Update hover state for both halves of the row given the cursor.
    pub fn drive_hover(&mut self, mouse: (f32, f32), bounds: Rect) {
        let (input, browse) = split_bounds(bounds);
        self.hover_input = input.contains(Point::new(mouse.0, mouse.1));
        self.browse.hover = browse.contains(Point::new(mouse.0, mouse.1));
    }

    /// React to a press — returns `true` if the Browse button was clicked.
    pub fn handle_press(&mut self, mouse: (f32, f32), bounds: Rect) -> bool {
        let (_, browse) = split_bounds(bounds);
        self.browse.handle(mouse, browse, true)
    }
}

/// Split the path-field row into `(input_rect, browse_rect)`.
pub fn split_bounds(bounds: Rect) -> (Rect, Rect) {
    let browse = Rect::from_xywh(
        bounds.right - BROWSE_W,
        bounds.top + (bounds.height() - BROWSE_H) * 0.5,
        BROWSE_W,
        BROWSE_H,
    );
    let input = Rect::from_ltrb(
        bounds.left,
        bounds.top,
        browse.left - ROW_GAP,
        bounds.bottom,
    );
    (input, browse)
}

/// Path-field row default height — matches a Browse button + a couple of px.
pub const ROW_HEIGHT: f32 = 38.0;

/// Browse button bounds split out from the row — main.rs uses this to
/// route hover/press events without re-implementing the layout split.
pub fn browse_bounds(row: Rect) -> Rect {
    split_bounds(row).1
}

pub fn draw_vpathfield(canvas: &Canvas, bounds: Rect, state: &VpathfieldState, fonts: &FontStore) {
    let (input, browse) = split_bounds(bounds);
    draw_input(canvas, input, &state.value, state.hover_anim, fonts);
    draw_vghost_btn(canvas, browse, BROWSE_LABEL, &state.browse, GhostKind::Pearl, fonts);
}

fn draw_input(canvas: &Canvas, bounds: Rect, value: &str, hover_anim: f32, fonts: &FontStore) {
    let rrect = RRect::new_rect_xy(bounds, FIELD_RADIUS, FIELD_RADIUS);
    let anim = CubicBezier::SILK.eval(hover_anim.clamp(0.0, 1.0));

    // Background fill `rgba(229, 184, 197, 0.04)`
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.04),
        None,
    );
    canvas.draw_rrect(rrect, &bg);

    // Hover/focus glow ramp — CSS focus state shows `0 0 0 3px rgba(229,184,197,0.08)`.
    if anim > 0.001 {
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_style(PaintStyle::Stroke);
        glow.set_stroke_width(3.0);
        glow.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08 * anim),
            None,
        );
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 1.5, false));
        canvas.draw_rrect(rrect, &glow);
    }

    // Border — alpha lifts from 0.14 → 0.45 on focus/hover.
    let border_alpha = 0.14 + (0.45 - 0.14) * anim;
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, border_alpha),
        None,
    );
    canvas.draw_rrect(rrect, &border);

    // Value text (JetBrains Mono 12, pearl). Truncate from the left with an
    // ellipsis so the *tail* of long paths stays readable — that's where
    // the meaningful filename lives.
    let font = fonts.jetbrains_mono(12.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
    let max_width = bounds.width() - 2.0 * FIELD_PAD_X;
    let display = ellipsize_left(&font, value, max_width, &paint);
    let (_, m) = font.metrics();
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    canvas.draw_str(&display, (bounds.left + FIELD_PAD_X, baseline), &font, &paint);
}

/// Truncate from the left edge with a leading `…` so trailing characters
/// remain visible. `…/Some/Trailing/Path/file.json`.
fn ellipsize_left(font: &skia_safe::Font, s: &str, max_width: f32, paint: &Paint) -> String {
    let (full_w, _) = font.measure_str(s, Some(paint));
    if full_w <= max_width {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    // Binary search the cutoff index.
    let mut lo = 0_usize;
    let mut hi = chars.len();
    let prefix = '…';
    while lo < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = std::iter::once(prefix).chain(chars[mid..].iter().copied()).collect();
        let (w, _) = font.measure_str(&candidate, Some(paint));
        if w <= max_width {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    if lo >= chars.len() {
        return prefix.to_string();
    }
    std::iter::once(prefix).chain(chars[lo..].iter().copied()).collect()
}
