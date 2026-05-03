//! About dialog — small read-only modal showing the launcher's name,
//! version, and credits. Triggered by the "About" link on the main menu.
//!
//! Same chrome pattern as `new_instance_modal` (shroud + glass card +
//! tint + hairline rim) but smaller and with no form. Dismisses on
//! Esc / outside-click / Close button.

use ewo_core::CubicBezier;
use skia_safe::{
    canvas::SaveLayerRec, gradient_shader, image_filters, BlurStyle, Canvas, ClipOp, Color,
    Color4f, Contains, MaskFilter, Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

use crate::text::{self, FontStore};
use crate::widgets::{draw_vghost_btn, GhostKind, VghostBtnState};

const CARD_RADIUS: f32 = 22.0;
const CARD_W: f32 = 480.0;
const CARD_H: f32 = 380.0;
const PAD_X: f32 = 40.0;
const PAD_TOP: f32 = 36.0;
const PAD_BOTTOM: f32 = 28.0;
const ANIM_DURATION: f32 = 0.24;

const TEXT_PEARL_HOT: Color = Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0);
const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const ACCENT_BERRY: Color = Color::from_argb(0xFF, 0xB4, 0x74, 0x91);

#[derive(Debug, Clone, Default)]
pub struct AboutModalState {
    pub open: bool,
    /// 0..1, drives the entrance animation.
    pub anim: f32,
    pub close_btn: VghostBtnState,
}

impl AboutModalState {
    pub fn open(&mut self) {
        self.open = true;
        self.anim = 0.0;
        self.close_btn = VghostBtnState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.anim = 0.0;
    }

    pub fn tick(&mut self, dt: f32) {
        if self.open && self.anim < 1.0 {
            self.anim = (self.anim + dt / ANIM_DURATION).min(1.0);
        }
        self.close_btn.tick(dt);
    }
}

pub fn card_rect(card_w: f32, card_h: f32) -> Rect {
    let w = CARD_W.min(card_w - 80.0);
    let h = CARD_H.min(card_h - 80.0);
    let cx = card_w * 0.5;
    let cy = card_h * 0.5;
    Rect::from_xywh(cx - w * 0.5, cy - h * 0.5, w, h)
}

pub fn close_button_bounds(card_w: f32, card_h: f32) -> Rect {
    let card = card_rect(card_w, card_h);
    let btn_w: f32 = 100.0;
    let btn_h: f32 = 38.0;
    let cx = (card.left + card.right) * 0.5;
    Rect::from_xywh(
        cx - btn_w * 0.5,
        card.bottom - PAD_BOTTOM - btn_h,
        btn_w,
        btn_h,
    )
}

pub fn shroud_consumes(mouse: (f32, f32), card_w: f32, card_h: f32) -> bool {
    !card_rect(card_w, card_h).contains(Point::new(mouse.0, mouse.1))
}

pub fn draw_modal(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    state: &AboutModalState,
) {
    if !state.open && state.anim <= 0.001 {
        return;
    }
    let anim = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    // Shroud
    let shroud_alpha = (state.anim / 0.67).clamp(0.0, 1.0);
    draw_shroud(canvas, card_w, card_h, shroud_alpha);

    // Card with entrance transform
    let card = card_rect(card_w, card_h);
    let cx = (card.left + card.right) * 0.5;
    let cy = (card.top + card.bottom) * 0.5;
    let scale = 0.97 + 0.03 * anim;
    let ty = 10.0 * (1.0 - anim);

    let saved = canvas.save();
    canvas.translate((cx, cy + ty));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy));

    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(anim);
    canvas.save_layer(
        &SaveLayerRec::default()
            .bounds(&card)
            .paint(&layer_paint),
    );

    draw_card(canvas, fonts, &card, state);

    canvas.restore();
    canvas.restore_to_count(saved);
}

fn draw_shroud(canvas: &Canvas, card_w: f32, card_h: f32, alpha: f32) {
    let bounds = Rect::from_xywh(0.0, 0.0, card_w, card_h);
    if let Some(blur) = image_filters::blur((2.0, 2.0), TileMode::Clamp, None, None) {
        let saved = canvas.save();
        canvas.clip_rect(bounds, ClipOp::Intersect, true);
        let rec = SaveLayerRec::default().bounds(&bounds).backdrop(&blur);
        canvas.save_layer(&rec);

        let cx = card_w * 0.5;
        let cy = card_h * 0.5;
        let radius = (card_w.max(card_h)) * 0.7;
        if let Some(shader) = gradient_shader::radial(
            Point::new(cx, cy),
            radius,
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(8.0 / 255.0, 2.0 / 255.0, 6.0 / 255.0, 0.55 * alpha),
                    Color4f::new(8.0 / 255.0, 2.0 / 255.0, 6.0 / 255.0, 0.85 * alpha),
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
            p.set_shader(shader);
            canvas.draw_rect(bounds, &p);
        }

        canvas.restore();
        canvas.restore_to_count(saved);
    }
}

fn draw_card(canvas: &Canvas, fonts: &FontStore, card: &Rect, state: &AboutModalState) {
    let rrect = RRect::new_rect_xy(*card, CARD_RADIUS, CARD_RADIUS);

    // Drop shadow
    let mut s1 = Paint::default();
    s1.set_anti_alias(true);
    s1.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    s1.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 40.0, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(card.with_offset((0.0, 32.0)), CARD_RADIUS, CARD_RADIUS),
        &s1,
    );
    let mut s2 = Paint::default();
    s2.set_anti_alias(true);
    s2.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.4), None);
    s2.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 14.0, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(card.with_offset((0.0, 8.0)), CARD_RADIUS, CARD_RADIUS),
        &s2,
    );

    // Backdrop blur + dark wine fill
    if let Some(blur) = image_filters::blur((20.0, 20.0), TileMode::Clamp, None, None) {
        let saved = canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
        let rec = SaveLayerRec::default().bounds(card).backdrop(&blur);
        canvas.save_layer(&rec);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(card.left, card.top), Point::new(card.left, card.bottom)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(38.0 / 255.0, 18.0 / 255.0, 30.0 / 255.0, 0.92),
                    Color4f::new(28.0 / 255.0, 12.0 / 255.0, 22.0 / 255.0, 0.95),
                ],
                None,
            ),
            Some(&[0.0_f32, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
            canvas.draw_rrect(rrect, &fill);
        }
        canvas.restore();
        canvas.restore_to_count(saved);
    }

    // 135° tint + warm-white top fade
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(card.left, card.top),
            Point::new(card.right, card.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
                Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, 0.06),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 0.10),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.40, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        canvas.draw_rrect(rrect, &p);
    }

    // Hairline rim
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.18),
        None,
    );
    let inset = card.with_inset((0.5, 0.5));
    canvas.draw_rrect(
        RRect::new_rect_xy(inset, CARD_RADIUS - 0.5, CARD_RADIUS - 0.5),
        &rim,
    );

    // Inner content
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
    draw_inner(canvas, fonts, card, state);
    canvas.restore_to_count(saved);
}

fn draw_inner(canvas: &Canvas, fonts: &FontStore, card: &Rect, state: &AboutModalState) {
    let content_left = card.left + PAD_X;
    let content_right = card.right - PAD_X;
    let content_top = card.top + PAD_TOP;
    let cx = (content_left + content_right) * 0.5;

    // Eyebrow
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eb_paint = Paint::default();
    eb_paint.set_anti_alias(true);
    eb_paint.set_color(ACCENT_BERRY);
    let (_, em) = eyebrow_font.metrics();
    let eb_baseline = content_top + (-em.ascent);
    let eb_label = "ABOUT";
    let eb_w = text::measure_tracked_em(&eyebrow_font, eb_label, 0.18);
    text::draw_tracked_em(
        canvas,
        eb_label,
        (cx - eb_w * 0.5, eb_baseline),
        &eyebrow_font,
        &eb_paint,
        0.18,
    );

    // Title — Fraunces 48 "EwoClient"
    let title_font = fonts.fraunces_axes(48.0, 50.0, 1.0, 300.0, None);
    let mut t_paint = Paint::default();
    t_paint.set_anti_alias(true);
    t_paint.set_color(TEXT_PEARL_HOT);
    let title = "EwoClient";
    let (t_advance, _) = title_font.measure_str(title, Some(&t_paint));
    let (_, tm) = title_font.metrics();
    let title_top = eb_baseline + em.descent + 12.0;
    let title_baseline = title_top + (-tm.ascent);
    canvas.draw_str(title, (cx - t_advance * 0.5, title_baseline), &title_font, &t_paint);

    // Version line — JetBrains Mono 10 tracked
    let ver_font = fonts.jetbrains_mono(10.0);
    let mut ver_paint = Paint::default();
    ver_paint.set_anti_alias(true);
    ver_paint.set_color(TEXT_MAUVE);
    let ver = "V0.1 · VELVET BUILD · OFFLINE";
    let ver_w = text::measure_tracked_em(&ver_font, ver, 0.30);
    let (_, vm) = ver_font.metrics();
    let ver_top = title_baseline + tm.descent + 8.0;
    let ver_baseline = ver_top + (-vm.ascent);
    text::draw_tracked_em(
        canvas,
        ver,
        (cx - ver_w * 0.5, ver_baseline),
        &ver_font,
        &ver_paint,
        0.30,
    );

    // Tagline — italic Newsreader 16
    let tag_font = fonts.newsreader(16.0);
    let mut tag_paint = Paint::default();
    tag_paint.set_anti_alias(true);
    tag_paint.set_color(TEXT_PEARL);
    let tag = "— a place to wait in the dark —";
    let (tag_advance, _) = tag_font.measure_str(tag, Some(&tag_paint));
    let (_, tagm) = tag_font.metrics();
    let tag_top = ver_baseline + vm.descent + 28.0;
    let tag_baseline = tag_top + (-tagm.ascent);
    canvas.draw_str(tag, (cx - tag_advance * 0.5, tag_baseline), &tag_font, &tag_paint);

    // Credits — three centered mono 10 tracked lines
    let cred_font = fonts.jetbrains_mono(10.0);
    let mut cred_paint = Paint::default();
    cred_paint.set_anti_alias(true);
    cred_paint.set_color(TEXT_MAUVE_DEEP);
    let (_, cm) = cred_font.metrics();
    let cred_lines = [
        "BUILT WITH RUST + SKIA",
        "FRAUNCES · NEWSREADER · JETBRAINS MONO",
        "OFFLINE FIRST. NOTHING PHONES HOME.",
    ];
    let mut cy_credits = tag_baseline + tagm.descent + 28.0;
    for line in cred_lines.iter() {
        let w = text::measure_tracked_em(&cred_font, line, 0.30);
        let baseline = cy_credits + (-cm.ascent);
        text::draw_tracked_em(
            canvas,
            line,
            (cx - w * 0.5, baseline),
            &cred_font,
            &cred_paint,
            0.30,
        );
        cy_credits = baseline + cm.descent + 8.0;
    }

    // Close button — ghost pearl, centered at the bottom.
    let btn_w: f32 = 100.0;
    let btn_h: f32 = 38.0;
    let bcx = (card.left + card.right) * 0.5;
    let close_rect = Rect::from_xywh(
        bcx - btn_w * 0.5,
        card.bottom - PAD_BOTTOM - btn_h,
        btn_w,
        btn_h,
    );
    draw_vghost_btn(canvas, close_rect, "Close", &state.close_btn, GhostKind::Pearl, fonts);
}
