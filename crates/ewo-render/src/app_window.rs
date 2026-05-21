//! The app-window draw — outer rounded card with the prototype's 3-layer
//! box-shadow stack, the inset hairline rim, and the inner berry glow.
//!
//! Build-sequence step 2 introduced this; step 3 split it into outer/inner
//! halves so the backdrop can render in between (clipped to the card rrect).
//!
//! CSS reference:
//! ```css
//! .app-window {
//!   border-radius: 22px;
//!   box-shadow:
//!     0 40px 120px -20px rgba(180, 100, 140, 0.35),
//!     0 12px 40px -8px rgba(0, 0, 0, 0.8),
//!     inset 0 0 0 1px rgba(229, 184, 197, 0.08);
//! }
//! .app-window::after {
//!   box-shadow: inset 0 0 80px -20px rgba(180, 100, 140, 0.15);
//! }
//! ```

use ewo_core::{Screen, Settings, Theme};
use skia_safe::{
    BlurStyle, Canvas, ClipOp, Color, MaskFilter, Paint, PaintStyle, RRect, Rect,
};

use crate::backdrop::Backdrop;
use crate::screens;
use crate::text::{FontStore, HoverGlowState};
use crate::widgets::VbtnState;

/// CSS card inset — distance from window edge to card edge.
pub const CARD_INSET: f32 = 28.0;
/// Card corner radius.
pub const CARD_RADIUS: f32 = 22.0;

/// CSS-blur-radius → Skia blur sigma.
fn css_blur_to_sigma(css_blur_px: f32) -> f32 {
    css_blur_px * 0.5
}

/// Compute the rounded-rect bounds of the card given the viewport size.
pub fn card_rrect(viewport_w: f32, viewport_h: f32) -> RRect {
    let rect = Rect::from_xywh(
        CARD_INSET,
        CARD_INSET,
        viewport_w - 2.0 * CARD_INSET,
        viewport_h - 2.0 * CARD_INSET,
    );
    RRect::new_rect_xy(rect, CARD_RADIUS, CARD_RADIUS)
}

/// Card content size in card-local coordinates given the viewport size.
pub fn card_content_size(viewport_w: u32, viewport_h: u32) -> (f32, f32) {
    (
        viewport_w as f32 - 2.0 * CARD_INSET,
        viewport_h as f32 - 2.0 * CARD_INSET,
    )
}

/// Render one full frame: chrome → backdrop → screen content → chrome trim.
pub fn draw_frame(
    canvas: &Canvas,
    backdrop: &Backdrop,
    fonts: &FontStore,
    viewport_w: u32,
    viewport_h: u32,
    time: f32,
    theme: &Theme,
    settings: &Settings,
    screen: Screen,
    launch_button: &VbtnState,
    menu_states: &[VbtnState; 4],
    settings_tab: screens::SettingsTab,
    prefs: &screens::Prefs,
    instance_prefs: &screens::InstancePrefs,
    launching_state: &screens::LaunchingState,
    modal: &screens::NewInstanceModalState,
    about_modal: &screens::AboutModalState,
    dev_overlay: Option<&screens::DevOverlayState>,
    frame_stats: screens::FrameStats,
    instances: &[screens::instances::Instance],
    heading_hover: HoverGlowState,
    account: screens::settings::AccountView<'_>,
    profiles: screens::settings::ProfileView<'_>,
) {
    let w = viewport_w as f32;
    let h = viewport_h as f32;

    draw_chrome_outer(canvas, w, h);

    // Backdrop + screen content are drawn inside the card rrect, in card-local
    // coordinates so the gradients' "35% 40%" centers refer to the card not
    // the window.
    let saved = canvas.save();
    let rrect = card_rrect(w, h);
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
    canvas.translate((CARD_INSET, CARD_INSET));
    let card_w = w - 2.0 * CARD_INSET;
    let card_h = h - 2.0 * CARD_INSET;
    backdrop.draw(canvas, card_w, card_h, time, theme, settings);

    // Tab bar is shared across screens (drawn first so screen content can
    // overlap it deliberately if needed — currently nothing does).
    screens::draw_tab_bar(canvas, fonts, card_w, screen);

    // Screen-specific content.
    match screen {
        Screen::MainMenu => {
            screens::draw_main_menu(canvas, fonts, card_w, card_h, time, menu_states, heading_hover);
        }
        Screen::Instances => {
            screens::draw_instances(
                canvas,
                fonts,
                card_w,
                card_h,
                time,
                settings,
                launch_button,
                instance_prefs,
                instances,
            );
        }
        Screen::Settings => {
            screens::draw_settings(
                canvas, fonts, card_w, card_h, time, settings, settings_tab, prefs, account,
                profiles,
            );
        }
        Screen::Launching => {
            screens::draw_launching(
                canvas, fonts, card_w, card_h, time, settings, launching_state,
            );
        }
    }
    if !fonts.has_fraunces {
        log_fallback_once();
    }

    // Modal overlay (above all screen content, still inside the card clip).
    screens::draw_new_instance_modal(canvas, fonts, card_w, card_h, time, settings, modal);
    screens::draw_about_modal(canvas, fonts, card_w, card_h, about_modal);

    // Dev overlay sits above everything when --dev is enabled.
    if let Some(state) = dev_overlay {
        screens::draw_dev_overlay(
            canvas, fonts, card_w, card_h, time, settings, state, frame_stats,
        );
    }

    canvas.restore_to_count(saved);

    draw_chrome_inner(canvas, w, h);
}

fn log_fallback_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        log::warn!(
            "rendering heading with system-default typeface — drop a Fraunces \
             variable TTF in assets/fonts/ for visual parity."
        );
    });
}

/// Draw the chrome up to (but not including) the card body — i.e. the outer
/// box-shadows and a black fill of the card. The body fill matters because
/// the backdrop layers use screen-blend on top of it.
pub fn draw_chrome_outer(canvas: &Canvas, w: f32, h: f32) {
    canvas.clear(Color::BLACK);

    let card = Rect::from_xywh(CARD_INSET, CARD_INSET, w - 2.0 * CARD_INSET, h - 2.0 * CARD_INSET);

    // Outer berry glow: 0 40 120 -20 rgba(180, 100, 140, 0.35)
    draw_outer_shadow(
        canvas,
        &card,
        CARD_RADIUS,
        (0.0, 40.0),
        120.0,
        -20.0,
        Color::from_argb((0.35 * 255.0) as u8, 180, 100, 140),
    );

    // Outer black drop: 0 12 40 -8 rgba(0, 0, 0, 0.8)
    draw_outer_shadow(
        canvas,
        &card,
        CARD_RADIUS,
        (0.0, 12.0),
        40.0,
        -8.0,
        Color::from_argb((0.8 * 255.0) as u8, 0, 0, 0),
    );

    // Card body — fills with --bg-core black so screen-blend layers on top
    // have a defined source.
    let card_rrect_ = RRect::new_rect_xy(card, CARD_RADIUS, CARD_RADIUS);
    let mut body = Paint::default();
    body.set_anti_alias(true);
    body.set_color(Color::BLACK);
    canvas.draw_rrect(card_rrect_, &body);
}

/// Draw the inset hairline rim and the `::after` inner berry glow on top of
/// whatever was rendered into the card.
pub fn draw_chrome_inner(canvas: &Canvas, w: f32, h: f32) {
    let card = Rect::from_xywh(CARD_INSET, CARD_INSET, w - 2.0 * CARD_INSET, h - 2.0 * CARD_INSET);
    let card_rrect_ = RRect::new_rect_xy(card, CARD_RADIUS, CARD_RADIUS);

    // Inset hairline rim: inset 0 0 0 1px rgba(229, 184, 197, 0.08)
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color(Color::from_argb((0.08 * 255.0) as u8, 229, 184, 197));
    let inset_rect = card.with_inset((0.5, 0.5));
    let inset_rrect = RRect::new_rect_xy(inset_rect, CARD_RADIUS - 0.5, CARD_RADIUS - 0.5);
    canvas.draw_rrect(inset_rrect, &rim);

    // Inner berry glow: ::after inset 0 0 80px -20px rgba(180, 100, 140, 0.15)
    let saved = canvas.save();
    canvas.clip_rrect(card_rrect_, Some(ClipOp::Intersect), Some(true));
    let mut inner_glow = Paint::default();
    inner_glow.set_anti_alias(true);
    inner_glow.set_style(PaintStyle::Stroke);
    inner_glow.set_stroke_width(40.0);
    inner_glow.set_color(Color::from_argb((0.22 * 255.0) as u8, 180, 100, 140));
    inner_glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 40.0, false));
    canvas.draw_rrect(card_rrect_, &inner_glow);
    canvas.restore_to_count(saved);
}

fn draw_outer_shadow(
    canvas: &Canvas,
    card: &Rect,
    radius: f32,
    offset: (f32, f32),
    blur: f32,
    spread: f32,
    color: Color,
) {
    let shadow_rect = Rect::from_xywh(
        card.left + offset.0 - spread,
        card.top + offset.1 - spread,
        card.width() + 2.0 * spread,
        card.height() + 2.0 * spread,
    );
    let shadow_radius = (radius + spread).max(0.0);
    let shadow_rrect = RRect::new_rect_xy(shadow_rect, shadow_radius, shadow_radius);

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(color);
    if blur > 0.0 {
        paint.set_mask_filter(MaskFilter::blur(
            BlurStyle::Normal,
            css_blur_to_sigma(blur),
            false,
        ));
    }
    canvas.draw_rrect(shadow_rrect, &paint);
}

