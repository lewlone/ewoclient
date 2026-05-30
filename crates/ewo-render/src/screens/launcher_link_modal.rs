//! Launcher-link modal (Phase H2) — the user pastes the 6-digit code
//! produced by `/launcher-link` in-game. On submit, the launcher POSTs
//! `/api/launcher/link` and stores the returned `social_token` against
//! the active MS account.
//!
//! Same chrome as `about_modal`/`new_instance_modal` (shroud + glass
//! card + tint + hairline rim) so it doesn't feel like a different
//! widget vocabulary. Input is keyboard-only — digits typed by the user
//! flow through `App`'s key handler into `LauncherLinkModalState::push_digit`.

use ewo_core::{CubicBezier, Settings};
use skia_safe::{
    canvas::SaveLayerRec, gradient_shader, image_filters, Canvas, Color, Color4f, Contains, Font,
    Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

use crate::text::FontStore;
use crate::widgets::{draw_vbtn, draw_vghost_btn, GhostKind, VbtnState, VghostBtnState};

const CARD_RADIUS: f32 = 22.0;
const CARD_W: f32 = 480.0;
const CARD_H: f32 = 360.0;
const PAD_X: f32 = 36.0;
const PAD_TOP: f32 = 30.0;
const PAD_BOTTOM: f32 = 28.0;
const ANIM_DURATION: f32 = 0.24;
const CODE_DIGITS: usize = 6;

const TEXT_PEARL_HOT: Color = Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0);
const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const ACCENT_ROSE: Color = Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5);
const ACCENT_EMBER: Color = Color::from_argb(0xFF, 0xD4, 0x88, 0x9A);
const ACCENT_BERRY: Color = Color::from_argb(0xFF, 0xB4, 0x74, 0x91);
const ACCENT_LAV: Color = Color::from_argb(0xFF, 0xC9, 0xA5, 0xD4);

/// Renderer-side view of the in-flight redemption (mirror of the
/// launcher's `social::LinkRedeemStatus` — we don't import that here
/// because `ewo-render` doesn't depend on the launcher crate).
#[derive(Copy, Clone, Debug)]
pub enum LinkRedeemView<'a> {
    Idle,
    Submitting,
    /// User-facing error message (short).
    Failed(&'a str),
}

#[derive(Debug, Clone, Default)]
pub struct LauncherLinkModalState {
    pub open: bool,
    /// 0..1, drives the entrance animation. Matches the about modal.
    pub anim: f32,
    /// Digits the user has typed (max `CODE_DIGITS`).
    pub code: String,
    pub cancel_btn: VghostBtnState,
    pub submit_btn: VbtnState,
}

impl LauncherLinkModalState {
    pub fn open(&mut self) {
        self.open = true;
        self.anim = 0.0;
        self.code.clear();
        self.cancel_btn = VghostBtnState::default();
        self.submit_btn = VbtnState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.anim = 0.0;
    }

    pub fn tick(&mut self, dt: f32) {
        if self.open && self.anim < 1.0 {
            self.anim = (self.anim + dt / ANIM_DURATION).min(1.0);
        }
        self.cancel_btn.tick(dt);
        self.submit_btn.tick(dt);
    }

    /// Append an ASCII digit if there's space. Non-digit chars are
    /// ignored (the main-loop key handler is the gate, but we double-check).
    pub fn push_digit(&mut self, c: char) {
        if c.is_ascii_digit() && self.code.len() < CODE_DIGITS {
            self.code.push(c);
        }
    }

    pub fn pop_digit(&mut self) {
        self.code.pop();
    }

    /// Whether the code is complete (6 digits typed). The Submit button
    /// renders disabled until this is true.
    pub fn is_ready(&self) -> bool {
        self.code.len() == CODE_DIGITS
    }
}

pub fn card_rect(card_w: f32, card_h: f32) -> Rect {
    let w = CARD_W.min(card_w - 80.0);
    let h = CARD_H.min(card_h - 80.0);
    let cx = card_w * 0.5;
    let cy = card_h * 0.5;
    Rect::from_xywh(cx - w * 0.5, cy - h * 0.5, w, h)
}

pub fn cancel_button_bounds(card_w: f32, card_h: f32) -> Rect {
    let card = card_rect(card_w, card_h);
    let btn_w: f32 = 110.0;
    let btn_h: f32 = 38.0;
    Rect::from_xywh(
        card.left + PAD_X,
        card.bottom - PAD_BOTTOM - btn_h,
        btn_w,
        btn_h,
    )
}

pub fn submit_button_bounds(card_w: f32, card_h: f32) -> Rect {
    let card = card_rect(card_w, card_h);
    let btn_w: f32 = 130.0;
    let btn_h: f32 = 38.0;
    Rect::from_xywh(
        card.right - PAD_X - btn_w,
        card.bottom - PAD_BOTTOM - btn_h,
        btn_w,
        btn_h,
    )
}

/// True if the press at `mouse` lands on the shroud (outside the card).
/// Caller closes the modal in that case.
pub fn shroud_consumes(mouse: (f32, f32), card_w: f32, card_h: f32) -> bool {
    !card_rect(card_w, card_h).contains(Point::new(mouse.0, mouse.1))
}

pub fn draw_modal(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    time: f32,
    settings: &Settings,
    state: &LauncherLinkModalState,
    redeem: LinkRedeemView<'_>,
) {
    if !state.open && state.anim <= 0.001 {
        return;
    }
    let anim = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    // Shroud
    let shroud_alpha = (state.anim / 0.67).clamp(0.0, 1.0);
    draw_shroud(canvas, card_w, card_h, shroud_alpha);

    // Card entrance: translateY(10 → 0) + scale(0.97 → 1) + opacity 0 → 1.
    // No blur per CLAUDE.md non-negotiable #3 (text-bearing surface).
    canvas.save();
    let card = card_rect(card_w, card_h);
    let cx = (card.left + card.right) * 0.5;
    let cy = (card.top + card.bottom) * 0.5;
    let ty = (1.0 - anim) * 10.0;
    let scale = 0.97 + 0.03 * anim;
    canvas.translate((cx, cy + ty));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy));

    let alpha = anim;

    let mut card_paint = Paint::default();
    card_paint.set_anti_alias(true);
    card_paint.set_alpha_f(alpha);

    draw_card_chrome(canvas, &card, alpha);

    let rrect = RRect::new_rect_xy(card, CARD_RADIUS, CARD_RADIUS);
    canvas.save();
    canvas.clip_rrect(rrect, None, true);

    // Inner content.
    let content_left = card.left + PAD_X;
    let content_right = card.right - PAD_X;
    let content_w = content_right - content_left;

    // Eyebrow "LINK LAUNCHER".
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(scale_color(TEXT_MAUVE, alpha));
    let eyebrow_y = card.top + PAD_TOP + 12.0;
    canvas.draw_str("LINK LAUNCHER", (content_left, eyebrow_y), &eyebrow_font, &eyebrow_paint);

    // Title.
    let title_font = fonts.fraunces_axes(28.0, 50.0, 0.0, 600.0, None);
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color(scale_color(TEXT_PEARL_HOT, alpha));
    let (_, tm) = title_font.metrics();
    let title_y = eyebrow_y + 26.0 + (-tm.ascent);
    canvas.draw_str("Enter the code", (content_left, title_y), &title_font, &title_paint);

    // Subtitle (italic newsreader).
    let sub_font = Font::new(fonts.newsreader_italic_typeface().clone(), 13.0);
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color(scale_color(TEXT_MAUVE, alpha));
    let (_, sm) = sub_font.metrics();
    let sub_y = title_y + tm.descent + 14.0 + (-sm.ascent);
    canvas.draw_str(
        "run /launcher-link on chickenedin to get a 6-digit code.",
        (content_left, sub_y),
        &sub_font,
        &sub_paint,
    );

    // Digit slots — 6 boxes side-by-side, each shows a typed digit or "_".
    let slots_top = sub_y + sm.descent + 24.0;
    let slot_h: f32 = 56.0;
    let slot_gap: f32 = 10.0;
    let slot_w = (content_w - slot_gap * (CODE_DIGITS - 1) as f32) / CODE_DIGITS as f32;
    let typed: Vec<char> = state.code.chars().collect();
    for i in 0..CODE_DIGITS {
        let x = content_left + i as f32 * (slot_w + slot_gap);
        let r = Rect::from_xywh(x, slots_top, slot_w, slot_h);
        let active = i == typed.len().min(CODE_DIGITS - 1) && typed.len() < CODE_DIGITS;
        draw_digit_slot(canvas, fonts, &r, typed.get(i).copied(), active, alpha);
    }

    // Status line below the slots — shows submit/error state from social.
    let status_y = slots_top + slot_h + 18.0;
    let status_font = fonts.newsreader(13.0);
    let mut status_paint = Paint::default();
    status_paint.set_anti_alias(true);
    let (_, statm) = status_font.metrics();
    match redeem {
        LinkRedeemView::Idle => {}
        LinkRedeemView::Submitting => {
            status_paint.set_color(scale_color(TEXT_MAUVE, alpha));
            canvas.draw_str(
                "submitting…",
                (content_left, status_y + (-statm.ascent)),
                &status_font,
                &status_paint,
            );
        }
        LinkRedeemView::Failed(msg) => {
            status_paint.set_color(scale_color(ACCENT_EMBER, alpha));
            canvas.draw_str(
                msg,
                (content_left, status_y + (-statm.ascent)),
                &status_font,
                &status_paint,
            );
        }
    }

    canvas.restore(); // un-clip from rrect

    // Buttons. Cancel (ghost), Link (vbtn).
    let cancel = cancel_button_bounds(card_w, card_h);
    let submit = submit_button_bounds(card_w, card_h);

    draw_vghost_btn(canvas, cancel, "Cancel", &state.cancel_btn, GhostKind::Pearl, fonts);

    // Submit button label depends on state.
    let submit_label = match redeem {
        LinkRedeemView::Submitting => "Linking…",
        _ => "Link",
    };
    draw_vbtn(
        canvas,
        submit,
        submit_label,
        &state.submit_btn,
        time,
        settings.motion_speed,
        fonts,
        true,
    );

    canvas.restore(); // un-scale + translate
}

fn draw_shroud(canvas: &Canvas, card_w: f32, card_h: f32, alpha: f32) {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    let center = Point::new(card_w * 0.5, card_h * 0.5);
    let radius = (card_w.max(card_h)) * 0.7;
    let shader = gradient_shader::radial(
        center,
        radius,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(0.10, 0.0, 0.04, 0.55 * alpha),
                Color4f::new(0.0, 0.0, 0.0, 0.85 * alpha),
            ],
            None,
        ),
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    );
    if let Some(s) = shader {
        p.set_shader(s);
    } else {
        p.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.7 * alpha), None);
    }
    let filter = image_filters::blur((4.0, 4.0), None, None, None);
    if let Some(f) = filter {
        p.set_image_filter(f);
    }
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, card_w, card_h), &p);
}

fn draw_card_chrome(canvas: &Canvas, card: &Rect, alpha: f32) {
    let rrect = RRect::new_rect_xy(*card, CARD_RADIUS, CARD_RADIUS);

    // Drop shadow stack
    for (offset_y, sigma, color) in &[
        (24.0_f32, 28.0_f32, Color4f::new(0.0, 0.0, 0.0, 0.45 * alpha)),
        (10.0, 14.0, Color4f::new(0.0, 0.0, 0.0, 0.35 * alpha)),
        (0.0, 30.0, Color4f::new(0.71, 0.45, 0.57, 0.20 * alpha)),
    ] {
        let mut sh = Paint::default();
        sh.set_anti_alias(true);
        sh.set_color4f(*color, None);
        sh.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            *sigma,
            false,
        ));
        let shadow_rect = card.with_offset((0.0, *offset_y));
        canvas.draw_rrect(RRect::new_rect_xy(shadow_rect, CARD_RADIUS, CARD_RADIUS), &sh);
    }

    // Backdrop blur (refract) — capture what's beneath through a blur.
    let blur_filter = image_filters::blur((20.0, 20.0), None, None, None);
    if let Some(bf) = blur_filter {
        let layer_rec = SaveLayerRec::default().bounds(card).backdrop(&bf);
        canvas.save_layer(&layer_rec);
        // Dark wine fill on top of the refract.
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(Color4f::new(0.11, 0.04, 0.08, 0.92 * alpha), None);
        canvas.draw_rrect(rrect, &fill);
        canvas.restore();
    }

    // Top warm-radial tint
    if let Some(s) = gradient_shader::radial(
        Point::new((card.left + card.right) * 0.5, card.top),
        (card.width() * 0.6).max(card.height() * 0.5),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(1.0, 0.96, 0.94, 0.10 * alpha),
                Color4f::new(1.0, 0.96, 0.94, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(s);
        canvas.draw_rrect(rrect, &p);
    }

    // Hairline rim
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(0.90, 0.72, 0.77, 0.18 * alpha),
        None,
    );
    canvas.draw_rrect(rrect, &rim);
}

fn draw_digit_slot(
    canvas: &Canvas,
    fonts: &FontStore,
    r: &Rect,
    digit: Option<char>,
    active: bool,
    alpha: f32,
) {
    let rrect = RRect::new_rect_xy(*r, 10.0, 10.0);

    // Faint rose fill
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(Color4f::new(0.90, 0.72, 0.77, 0.05 * alpha), None);
    canvas.draw_rrect(rrect, &bg);

    // Border — rose, brighter on the active slot
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(if active { 1.6 } else { 1.0 });
    let border_alpha = if active { 0.55 } else { 0.22 };
    border.set_color4f(Color4f::new(0.90, 0.72, 0.77, border_alpha * alpha), None);
    canvas.draw_rrect(rrect, &border);

    // Digit (or empty marker)
    let glyph_font = fonts.fraunces_axes(34.0, 30.0, 0.0, 500.0, None);
    let mut glyph_paint = Paint::default();
    glyph_paint.set_anti_alias(true);
    let (_, gm) = glyph_font.metrics();
    let baseline = (r.top + r.bottom) * 0.5 + (-gm.ascent - gm.descent) * 0.5;
    match digit {
        Some(d) => {
            glyph_paint.set_color(scale_color(TEXT_PEARL_HOT, alpha));
            let s = d.to_string();
            let (advance, _) = glyph_font.measure_str(&s, Some(&glyph_paint));
            let x = (r.left + r.right) * 0.5 - advance * 0.5;
            canvas.draw_str(&s, (x, baseline), &glyph_font, &glyph_paint);
        }
        None => {
            glyph_paint.set_color(scale_color(TEXT_MAUVE_DEEP, alpha * 0.7));
            // A faint horizontal underline mid-slot as an empty marker.
            let mid_y = (r.top + r.bottom) * 0.5 + 8.0;
            let mut line = Paint::default();
            line.set_anti_alias(true);
            line.set_stroke_width(1.0);
            line.set_style(PaintStyle::Stroke);
            line.set_color4f(Color4f::new(0.60, 0.50, 0.55, 0.45 * alpha), None);
            canvas.draw_line(
                (r.left + 16.0, mid_y),
                (r.right - 16.0, mid_y),
                &line,
            );
        }
    }
    // Suppress unused-variable warnings on the rose accents we deliberately
    // didn't end up needing at the slot level (the border carries the cue).
    let _ = ACCENT_ROSE;
    let _ = ACCENT_BERRY;
    let _ = ACCENT_LAV;
    let _ = TEXT_PEARL;
}

fn scale_color(c: Color, alpha: f32) -> Color {
    let a = ((c.a() as f32) * alpha.clamp(0.0, 1.0)) as u8;
    Color::from_argb(a, c.r(), c.g(), c.b())
}
