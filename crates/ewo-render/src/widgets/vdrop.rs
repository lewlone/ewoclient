//! `vdrop` — the prototype's portaled dropdown.
//!
//! Two pieces:
//!   1. **Head** (`.vdrop-head.vbtn`) — sits inline inside its parent layout
//!      like a velvet button. Renders the current value + a chevron caret
//!      that flips when open.
//!   2. **Menu** (`.vdrop-portal-menu`) — opens as a portaled overlay
//!      anchored to the head. Animates in with a 220ms silk-eased
//!      opacity/scale/blur entrance, plus a 50ms-per-row staggered fade for
//!      list items. Flips up when there's not enough room below.
//!
//! Render path: caller draws the head with `draw_vdrop_head` while inside
//! whatever screen layout owns the row; *after* the screen's glass panel
//! returns (so the clip releases), caller calls `draw_vdrop_menu` for any
//! open dropdown so the menu can spill outside the panel.
//!
//! CSS reference: `.vdrop`, `.vdrop-head`, `.vdrop-caret`, `.vdrop-portal-menu`,
//! `.vdrop-panel-solid`, `.vdrop-row`, `.vdrop-dot` in `StyleSheet1`.

use ewo_core::{CubicBezier, Settings};
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, Color, Color4f, Contains, MaskFilter, Paint,
    PaintStyle, Path, Point, RRect, Rect, TileMode,
};

/// Per-row stagger delay (CSS `animation-delay: calc(var(--row-i) * 50ms)`).
const ROW_STAGGER_S: f32 = 0.05;
/// Per-row entrance duration (CSS `vdrop-row-in` keyframe block).
const ROW_REVEAL_S: f32 = 0.40;

use crate::text::FontStore;
use crate::widgets::vbtn;

const HEAD_RADIUS: f32 = 14.0;
const MENU_RADIUS: f32 = 14.0;
const ROW_HEIGHT: f32 = 38.0; // matches CSS `.vdrop-row` padding 10 + line-height
const ROW_PAD_X: f32 = 12.0;
const MENU_PAD: f32 = 6.0;
const MENU_GAP: f32 = 8.0; // CSS `top: calc(100% + 8px)`
/// Hard cap on the menu's visible height. Anything past this scrolls.
const MENU_MAX_VISIBLE_H: f32 = 280.0;

#[derive(Debug, Copy, Clone, Default)]
pub struct VdropState {
    pub open: bool,
    pub hover_head: bool,
    /// Index of the row currently under the cursor while the menu is open.
    pub hovered_row: Option<usize>,
    pub selected: usize,
    /// 0..1, tracks `open` over the CSS 220ms portal-in transition.
    pub anim: f32,
    /// Vertical scroll offset (pixels) for the menu rows when the
    /// content exceeds the visible bounds. Always `>= 0`.
    pub scroll: f32,
    /// Seconds since the menu transitioned `open = true`. Drives the
    /// per-row stagger (CSS `vdrop-row-in` with `animation-delay: i*50ms`).
    /// Reset to 0 each time the menu re-opens.
    pub time_open: f32,
}

impl VdropState {
    pub const fn new(selected: usize) -> Self {
        Self {
            open: false,
            hover_head: false,
            hovered_row: None,
            selected,
            anim: 0.0,
            scroll: 0.0,
            time_open: 0.0,
        }
    }

    /// Per-frame anim tick — drives the open/close fade and the row-stagger
    /// time accumulator. `time_open` resets the first frame after `open`
    /// flips true (detected via `anim ≈ 0`) so re-opening the same menu
    /// replays the stagger from the top.
    pub fn tick(&mut self, dt: f32) {
        if self.open {
            if self.anim <= 0.001 {
                self.time_open = 0.0;
            }
            self.time_open += dt;
        } else {
            self.time_open = 0.0;
        }
        let target = if self.open { 1.0 } else { 0.0 };
        let max_step = dt / 0.22; // 220ms portal-in
        let delta: f32 = target - self.anim;
        self.anim += delta.clamp(-max_step, max_step);
    }

    /// Handle a press on the dropdown head — toggles the menu open/closed.
    /// Returns `true` if open state flipped this frame.
    pub fn handle_head(
        &mut self,
        mouse: (f32, f32),
        head_bounds: Rect,
        mouse_just_pressed: bool,
    ) -> bool {
        let in_bounds = head_bounds.contains(Point::new(mouse.0, mouse.1));
        self.hover_head = in_bounds;
        if mouse_just_pressed && in_bounds {
            self.open = !self.open;
            return true;
        }
        false
    }

    /// Update hover-row state from cursor position. Pass the menu's
    /// computed bounds + how many rows it has. No-op when closed.
    pub fn update_menu_hover(&mut self, mouse: (f32, f32), menu_bounds: Rect, rows: usize) {
        if !self.open || rows == 0 {
            self.hovered_row = None;
            return;
        }
        let inside_x = mouse.0 >= menu_bounds.left + MENU_PAD
            && mouse.0 <= menu_bounds.right - MENU_PAD;
        // Cursor must also be inside the menu's visible vertical range —
        // anything outside the clipped area shouldn't highlight a row.
        let inside_y = mouse.1 >= menu_bounds.top && mouse.1 <= menu_bounds.bottom;
        if !inside_x || !inside_y {
            self.hovered_row = None;
            return;
        }
        let local_y = mouse.1 - (menu_bounds.top + MENU_PAD) + self.scroll;
        if local_y < 0.0 {
            self.hovered_row = None;
            return;
        }
        let idx = (local_y / ROW_HEIGHT) as usize;
        self.hovered_row = if idx < rows { Some(idx) } else { None };
    }

    /// Update the scroll offset by `delta_pixels` (positive = scroll down).
    /// Clamped to `[0, max]` where max is computed from the visible
    /// bounds + row count.
    pub fn scroll_by(&mut self, delta_pixels: f32, menu_bounds: Rect, rows: usize) {
        let max = menu_max_scroll(menu_bounds, rows);
        self.scroll = (self.scroll + delta_pixels).clamp(0.0, max);
    }

    /// Handle a press inside the open menu. If a row was clicked, returns
    /// the selected index (and the menu closes). If the click was outside
    /// the menu, returns `Some(self.selected)` after closing if you want
    /// "click-outside-to-dismiss"; this method only handles row clicks —
    /// outside-clicks are the caller's responsibility.
    pub fn handle_menu(
        &mut self,
        mouse: (f32, f32),
        menu_bounds: Rect,
        rows: usize,
        mouse_just_pressed: bool,
    ) -> Option<usize> {
        if !self.open || !mouse_just_pressed {
            return None;
        }
        self.update_menu_hover(mouse, menu_bounds, rows);
        if let Some(idx) = self.hovered_row {
            self.selected = idx;
            self.open = false;
            return Some(idx);
        }
        None
    }

    /// Force-close (used for click-outside dismissal in main.rs). Also
    /// resets scroll so the next open starts at the top.
    pub fn close(&mut self) {
        self.open = false;
        self.scroll = 0.0;
    }
}

// ────────────────────────────────────────────────────────────────────────
// Head — the inline button with current value + caret
// ────────────────────────────────────────────────────────────────────────

/// Draw the dropdown head. Mirrors `.vdrop-head.vbtn` — same tint + rim +
/// sheen as a vbtn, with the value text left-aligned and a caret on the right.
pub fn draw_vdrop_head(
    canvas: &Canvas,
    bounds: Rect,
    value: &str,
    state: &VdropState,
    time: f32,
    settings: &Settings,
    fonts: &FontStore,
) {
    let rrect = RRect::new_rect_xy(bounds, HEAD_RADIUS, HEAD_RADIUS);

    // Hover lift
    let lift_y = if state.hover_head { -0.5 } else { 0.0 };
    let saved = canvas.save();
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = (bounds.top + bounds.bottom) * 0.5;
    canvas.translate((cx, cy + lift_y));
    canvas.translate((-cx, -cy));

    // Layer 1: tint (reuse vbtn's gradient values; non-primary alpha set).
    vbtn::draw_tint_for_dropdown(canvas, bounds, HEAD_RADIUS);

    // Layer 2: rim — pulse anim shared with vbtn
    vbtn::draw_rim_for_dropdown(canvas, bounds, HEAD_RADIUS, time, settings.motion_speed, state.hover_head);

    // Layer 3: sheen — clipped to head
    let saved2 = canvas.save();
    canvas.clip_rrect(rrect, None, Some(true));
    vbtn::draw_sheen_for_dropdown(canvas, bounds, time, settings.motion_speed, state.hover_head);
    canvas.restore_to_count(saved2);

    // Layer 4: value text + caret
    draw_head_label_and_caret(canvas, bounds, value, state.anim, fonts);

    canvas.restore_to_count(saved);
}

fn draw_head_label_and_caret(
    canvas: &Canvas,
    bounds: Rect,
    value: &str,
    anim: f32,
    fonts: &FontStore,
) {
    // Value: Newsreader 14, pearl, vertically centered
    let font = fonts.newsreader(14.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
    let (_, m) = font.metrics();
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    canvas.draw_str(value, (bounds.left + 16.0, baseline), &font, &paint);

    // Caret — rotates 180° when open. Drawn as two strokes forming the lower-
    // right corner of a square (matches CSS border-right + border-bottom),
    // then rotated 45° at rest, -135° when open. We blend with `anim` so the
    // rotation tracks the open animation.
    let caret_cx = bounds.right - 16.0 - 6.0;
    let caret_cy = cy;
    let size = 6.0;

    // Rotation angle in radians: 45° at rest, -135° fully open.
    let from = std::f32::consts::FRAC_PI_4; // 45°
    let to = -3.0 * std::f32::consts::FRAC_PI_4; // -135°
    // `anim` already tracks the open/close direction, so the easing is
    // the same expression regardless of the menu's open state.
    let eased = CubicBezier::SILK.eval(anim.clamp(0.0, 1.0));
    let angle = from + (to - from) * eased;

    let saved = canvas.save();
    canvas.translate((caret_cx, caret_cy));
    canvas.rotate(angle.to_degrees(), None);

    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(1.5);
    p.set_color4f(Color4f::new(0xF4 as f32 / 255.0, 0xE8 as f32 / 255.0, 0xEA as f32 / 255.0, 0.7), None);
    let half = size * 0.5;
    // Two strokes: right edge and bottom edge of a small square.
    canvas.draw_line((half, -half), (half, half), &p);
    canvas.draw_line((-half, half), (half, half), &p);

    canvas.restore_to_count(saved);
}

// ────────────────────────────────────────────────────────────────────────
// Menu — portaled overlay
// ────────────────────────────────────────────────────────────────────────

/// Compute the menu's bounds + flip-up flag given the head bounds, the
/// number of options, and the card content extents.
///
/// **Always prefers down** — the menu opens below the head as long as
/// there's at least `MIN_DOWN_SPACE` of room. If the full content
/// doesn't fit, the height is capped (and the menu scrolls internally
/// via `state.scroll`). Only when the head is so close to the card's
/// bottom edge that even the minimum height doesn't fit does the menu
/// flip up.
///
/// `MENU_MAX_VISIBLE_H` further bounds the height so a 9-option list
/// doesn't dominate the screen even on tall windows.
pub fn menu_layout(head_bounds: Rect, rows: usize, card_h: f32) -> (Rect, bool) {
    let menu_w = head_bounds.width().max(220.0);
    let inner_h = (rows as f32) * ROW_HEIGHT;
    let desired_h = (inner_h + 2.0 * MENU_PAD).min(MENU_MAX_VISIBLE_H);

    let space_below = (card_h - 12.0 - (head_bounds.bottom + MENU_GAP)).max(0.0);
    let space_above = (head_bounds.top - 12.0 - MENU_GAP).max(0.0);

    // Strongly prefer opening DOWN. Only flip up when barely anything fits
    // below (< ~1.5 rows) and there's clearly more room above. A dropdown
    // that doesn't fully fit below opens down + scrolls rather than flipping —
    // the Graphics → Theme dropdown sits near the panel bottom and should
    // still open downward, not cover the rows above it.
    let flip_up = space_below < 1.5 * ROW_HEIGHT && space_above > space_below + 40.0;

    let (top, h) = if flip_up {
        let h = desired_h.min(space_above);
        let bottom = head_bounds.top - MENU_GAP;
        (bottom - h, h)
    } else {
        let h = desired_h.min(space_below);
        (head_bounds.bottom + MENU_GAP, h)
    };

    (
        Rect::from_xywh(head_bounds.left, top.max(12.0), menu_w, h),
        flip_up,
    )
}

/// Total content height of all menu rows + padding. Used by callers to
/// compute the maximum scroll offset.
pub fn menu_content_height(rows: usize) -> f32 {
    (rows as f32) * ROW_HEIGHT + 2.0 * MENU_PAD
}

/// Maximum scroll offset for a menu with the given visible bounds + row
/// count. Always `>= 0`.
pub fn menu_max_scroll(visible_bounds: Rect, rows: usize) -> f32 {
    (menu_content_height(rows) - visible_bounds.height()).max(0.0)
}

/// Draw the open menu. `state.anim` is expected to be ≥ 0; when 0 the
/// menu is fully closed and nothing draws. Caller should call this only
/// when `state.open || state.anim > 0` (so the close animation can play
/// out before the rect fully disappears).
pub fn draw_vdrop_menu(
    canvas: &Canvas,
    bounds: Rect,
    flip_up: bool,
    options: &[&str],
    state: &VdropState,
    fonts: &FontStore,
) {
    if state.anim <= 0.001 {
        return;
    }
    let anim = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    // Portal-in transform: scale 0.98 → 1, translateY(-4 → 0), opacity 0 → 1.
    // For flip-up, translateY starts at +4 (sliding up) but transform-origin
    // is at the bottom — Skia doesn't have transform-origin, so we approximate
    // by translating relative to the appropriate anchor.
    let scale = 0.98 + 0.02 * anim;
    let ty = if flip_up { 4.0 * (1.0 - anim) } else { -4.0 * (1.0 - anim) };

    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = if flip_up { bounds.bottom } else { bounds.top };

    let saved = canvas.save();
    canvas.translate((cx, cy));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy + ty));

    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(anim);
    canvas.save_layer(
        &skia_safe::canvas::SaveLayerRec::default()
            .bounds(&bounds)
            .paint(&layer_paint),
    );

    draw_menu_panel(canvas, bounds);
    draw_menu_rows(canvas, bounds, options, state, fonts);

    // Scrollbar — only renders when content > visible bounds.
    let content_h = menu_content_height(options.len());
    crate::widgets::draw_scrollbar(canvas, bounds, state.scroll, content_h);

    canvas.restore();
    canvas.restore_to_count(saved);
}

fn draw_menu_panel(canvas: &Canvas, bounds: Rect) {
    let rrect = RRect::new_rect_xy(bounds, MENU_RADIUS, MENU_RADIUS);

    // Drop shadow `0 18px 48px rgba(0,0,0,0.6)` + `0 4px 16px rgba(0,0,0,0.4)`
    // + warm bloom `0 0 32px rgba(180,116,145,0.12)`.
    // Soft drop shadow, CLIPPED to outside the panel's square footprint. The
    // panel is semi-transparent, so any shadow drawn behind it shows through
    // the rounded bottom-corner *cutouts* — that's what read as dark square
    // corners. Excluding the panel's bounding box from the clip means the
    // shadow can only render below/around the panel, never inside the corner
    // cutouts. `respect_ctm = true` keeps the blur DPI-correct on HiDPI.
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 22.0, true));
    let shadow_rect = bounds.with_offset((0.0, 14.0));
    let shadow_save = canvas.save();
    canvas.clip_rect(bounds, skia_safe::ClipOp::Difference, true);
    canvas.draw_rrect(
        RRect::new_rect_xy(shadow_rect, MENU_RADIUS, MENU_RADIUS),
        &shadow,
    );
    canvas.restore_to_count(shadow_save);

    let mut bloom = Paint::default();
    bloom.set_anti_alias(true);
    bloom.set_color4f(
        Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, 0.12),
        None,
    );
    bloom.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 16.0, true));
    canvas.draw_rrect(rrect, &bloom);

    // Solid panel body — vertical gradient `rgba(38,18,30,0.96)` →
    // `rgba(28,12,22,0.97)`.
    let Some(body) = gradient_shader::linear(
        (
            Point::new(bounds.left, bounds.top),
            Point::new(bounds.left, bounds.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(38.0 / 255.0, 18.0 / 255.0, 30.0 / 255.0, 0.96),
                Color4f::new(28.0 / 255.0, 12.0 / 255.0, 22.0 / 255.0, 0.97),
            ],
            None,
        ),
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) else {
        return;
    };
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(body);
    canvas.draw_rrect(rrect, &p);

    // ::before warm radial fade `120% 80% at 50% -10%, rose 0.10 → transparent 60%`
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = bounds.top - bounds.height() * 0.10;
    if let Some(rad) = gradient_shader::radial(
        Point::new(cx, cy),
        bounds.width() * 0.6,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut radp = Paint::default();
        radp.set_anti_alias(true);
        radp.set_shader(rad);
        canvas.draw_rrect(rrect, &radp);
    }

    // Inset hairline rim
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.16),
        None,
    );
    let inset = bounds.with_inset((0.5, 0.5));
    canvas.draw_rrect(
        RRect::new_rect_xy(inset, MENU_RADIUS - 0.5, MENU_RADIUS - 0.5),
        &rim,
    );

    // Top inner highlight `inset 0 1px 0 rgba(244, 232, 234, 0.06)` —
    // approximated as a 1px line just inside the top edge.
    let mut top_line = Paint::default();
    top_line.set_anti_alias(true);
    top_line.set_color4f(
        Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.06),
        None,
    );
    canvas.draw_line(
        (bounds.left + MENU_RADIUS, bounds.top + 1.0),
        (bounds.right - MENU_RADIUS, bounds.top + 1.0),
        &top_line,
    );
}

fn draw_menu_rows(
    canvas: &Canvas,
    bounds: Rect,
    options: &[&str],
    state: &VdropState,
    fonts: &FontStore,
) {
    let row_left = bounds.left + MENU_PAD;
    let row_right = bounds.right - MENU_PAD;
    let label_font = fonts.newsreader(14.0);
    let (_, m) = label_font.metrics();

    // Clip rows to the menu's visible bounds + translate by -scroll so
    // the visible window slides over the row stack.
    let clip_rrect = RRect::new_rect_xy(bounds, MENU_RADIUS, MENU_RADIUS);
    let saved = canvas.save();
    canvas.clip_rrect(clip_rrect, None, Some(true));
    canvas.translate((0.0, -state.scroll));

    let mut y = bounds.top + MENU_PAD;

    for (i, opt) in options.iter().enumerate() {
        // Skip rows entirely outside the visible band — small optimization.
        let visible_top = bounds.top + state.scroll - ROW_HEIGHT;
        let visible_bottom = bounds.bottom + state.scroll;
        if y + ROW_HEIGHT < visible_top || y > visible_bottom {
            y += ROW_HEIGHT;
            continue;
        }
        let row_rect = Rect::from_ltrb(row_left, y, row_right, y + ROW_HEIGHT);
        let row_rrect = RRect::new_rect_xy(row_rect, 10.0, 10.0);

        // Per-row stagger reveal (CSS `vdrop-row-in`): opacity 0→1 +
        // translateY(-4→0). The CSS also blurs 3px→0 on entrance, but we
        // deliberately skip that: non-negotiable #3 forbids blurring
        // text-bearing surfaces during entrance, and the square-bounded
        // (Decal) blur layer left a visible boxy artifact on the lower rows
        // while the menu was still animating in.
        let local = state.time_open - (i as f32) * ROW_STAGGER_S;
        let row_layer_handle = if state.open && local < ROW_REVEAL_S {
            let t = (local / ROW_REVEAL_S).clamp(0.0, 1.0);
            let eased = CubicBezier::SILK.eval(t.max(0.0));
            let alpha = if local < 0.0 { 0.0 } else { eased };
            let dy = (1.0 - eased) * -4.0; // -4 → 0
            let bounds_for_layer = Rect::from_xywh(
                row_left - 4.0,
                y - 4.0,
                row_right - row_left + 8.0,
                ROW_HEIGHT + 8.0,
            );
            let s = canvas.save_layer_alpha_f(bounds_for_layer, alpha);
            canvas.translate((0.0, dy));
            Some(s)
        } else {
            None
        };

        // Hover highlight
        if state.hovered_row == Some(i) {
            let mut hp = Paint::default();
            hp.set_anti_alias(true);
            hp.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.05),
                None,
            );
            canvas.draw_rrect(row_rrect, &hp);
        }

        // Divider above (skip first)
        if i > 0 {
            let dy = y;
            let mut div_paint = Paint::default();
            div_paint.set_anti_alias(true);
            div_paint.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.15),
                None,
            );
            // Faux gradient fade-in/out via a clip + alpha — close enough at
            // this scale that a flat low-alpha line reads correctly.
            canvas.draw_line((row_left + 8.0, dy), (row_right - 8.0, dy), &div_paint);
        }

        // Selected dot — 4×4 pearl, glows.
        let dot_cx = row_left + ROW_PAD_X + 2.0;
        let dot_cy = y + ROW_HEIGHT * 0.5;
        if i == state.selected {
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.8),
                None,
            );
            glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
            canvas.draw_circle((dot_cx, dot_cy), 3.0, &glow);

            let mut dot = Paint::default();
            dot.set_anti_alias(true);
            if let Some(shader) = gradient_shader::radial(
                Point::new(dot_cx - 0.5, dot_cy - 0.5),
                3.0,
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[
                        Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 1.0),
                        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                    ],
                    None,
                ),
                Some(&[0.0_f32, 0.7][..]),
                TileMode::Clamp,
                None,
                None,
            ) {
                dot.set_shader(shader);
                canvas.draw_circle((dot_cx, dot_cy), 2.0, &dot);
            }
        }

        // Label
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
        let baseline = dot_cy + m.cap_height * 0.5;
        canvas.draw_str(*opt, (row_left + ROW_PAD_X + 14.0, baseline), &label_font, &paint);

        if let Some(s) = row_layer_handle {
            canvas.restore_to_count(s);
        }

        y += ROW_HEIGHT;
    }

    canvas.restore_to_count(saved);

    // Suppress unused warning — kept here in case row dividers are restored
    // with a proper masked gradient.
    let _ = Path::new();
}
