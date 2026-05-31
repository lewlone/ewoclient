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

use std::cell::RefCell;

use ewo_core::{Screen, Settings, Theme};
use skia_safe::{
    AlphaType, BlurStyle, Canvas, ClipOp, Color, Color4f, ColorType, Image, ImageInfo, MaskFilter,
    Paint, PaintStyle, RRect, Rect,
};

use crate::backdrop::Backdrop;
use crate::screens;
use crate::text::{FontStore, HoverGlowState};
use crate::widgets::VbtnState;

/// CSS card inset — distance from window edge to card edge.
// 0 — the card fills the whole (transparent) window, so the window's edges ARE
// the card's edges. (Was 28px, which left an invisible margin that still
// counted as the window for resize/drag — the OS thought the edge was out in
// empty space. No outer drop shadow now, since there's no margin to hold it.)
pub const CARD_INSET: f32 = 0.0;
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
    hovered_tab: Option<Screen>,
    launch_button: &VbtnState,
    menu_states: &[VbtnState; 4],
    settings_tab: screens::SettingsTab,
    prefs: &screens::Prefs,
    instance_prefs: &screens::InstancePrefs,
    launching_state: &screens::LaunchingState,
    modal: &screens::NewInstanceModalState,
    about_modal: &screens::AboutModalState,
    launcher_link_modal: &screens::LauncherLinkModalState,
    link_redeem: screens::LinkRedeemView<'_>,
    dev_overlay: Option<&screens::DevOverlayState>,
    frame_stats: screens::FrameStats,
    instances: &[screens::instances::Instance],
    heading_hover: HoverGlowState,
    account: screens::settings::AccountView<'_>,
    profiles: screens::settings::ProfileView<'_>,
    keybinds: screens::settings::KeybindView<'_>,
    friends_prefs: &screens::FriendsPrefs,
    friends_view: screens::FriendsViewState<'_>,
    server_widget: screens::ServerWidgetView<'_>,
    main_menu_enter: f32,
    back_link_hover: bool,
    min_btn_hover: bool,
    close_btn_hover: bool,
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
    screens::draw_tab_bar(canvas, fonts, card_w, screen, hovered_tab);

    // Screen-specific content.
    match screen {
        Screen::MainMenu => {
            screens::draw_main_menu(
                canvas, fonts, card_w, card_h, time, menu_states, heading_hover, server_widget,
                main_menu_enter,
            );
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
                back_link_hover,
            );
        }
        Screen::Friends => {
            let _ = screens::draw_friends(
                canvas, fonts, card_w, card_h, time, friends_prefs, friends_view,
            );
        }
        Screen::Settings => {
            screens::draw_settings(
                canvas, fonts, card_w, card_h, time, settings, settings_tab, prefs, account,
                profiles, keybinds, back_link_hover,
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
    screens::draw_launcher_link_modal(
        canvas,
        fonts,
        card_w,
        card_h,
        time,
        settings,
        launcher_link_modal,
        link_redeem,
    );

    // Dev overlay sits above everything when --dev is enabled.
    if let Some(state) = dev_overlay {
        screens::draw_dev_overlay(
            canvas, fonts, card_w, card_h, time, settings, state, frame_stats,
        );
    }

    canvas.restore_to_count(saved);

    draw_chrome_inner(canvas, w, h);
    draw_window_buttons(canvas, w, min_btn_hover, close_btn_hover);
}

/// Top-right minimize + close button rects. The card fills the window now, so
/// these are both card-local and window-local. Shared by the renderer and the
/// launcher's hit-testing (so a click on a button isn't treated as a drag).
pub fn window_button_bounds(width: f32) -> (Rect, Rect) {
    const SIZE: f32 = 28.0;
    const Y: f32 = 12.0;
    const GAP: f32 = 4.0;
    const RIGHT_PAD: f32 = 14.0;
    let close = Rect::from_xywh(width - RIGHT_PAD - SIZE, Y, SIZE, SIZE);
    let minimize = Rect::from_xywh(close.left - GAP - SIZE, Y, SIZE, SIZE);
    (minimize, close)
}

/// Velvet-themed minimize (—) + close (×) buttons in the top-right. Icons are
/// drawn as vector strokes (no glyph fonts). Hover fills a soft rounded
/// background — rose for minimize, ember for close — and brightens the icon.
fn draw_window_buttons(canvas: &Canvas, width: f32, min_hover: bool, close_hover: bool) {
    let (minimize, close) = window_button_bounds(width);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    if min_hover {
        bg.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.14), None);
        canvas.draw_rrect(RRect::new_rect_xy(minimize, 7.0, 7.0), &bg);
    }
    if close_hover {
        bg.set_color4f(Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.20), None);
        canvas.draw_rrect(RRect::new_rect_xy(close, 7.0, 7.0), &bg);
    }

    let icon = |hover: bool, danger: bool| -> Paint {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_style(PaintStyle::Stroke);
        p.set_stroke_width(1.5);
        p.set_color(if hover {
            if danger {
                Color::from_argb(0xFF, 0xF4, 0xC8, 0xD0)
            } else {
                Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA)
            }
        } else {
            Color::from_argb(0xFF, 0x9A, 0x80, 0x87)
        });
        p
    };

    // Minimize — a short centred horizontal bar.
    let mcx = (minimize.left + minimize.right) * 0.5;
    let mcy = (minimize.top + minimize.bottom) * 0.5;
    canvas.draw_line((mcx - 5.0, mcy), (mcx + 5.0, mcy), &icon(min_hover, false));

    // Close — an ×.
    let ccx = (close.left + close.right) * 0.5;
    let ccy = (close.top + close.bottom) * 0.5;
    let r = 5.0;
    let p = icon(close_hover, true);
    canvas.draw_line((ccx - r, ccy - r), (ccx + r, ccy + r), &p);
    canvas.draw_line((ccx - r, ccy + r), (ccx + r, ccy - r), &p);
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
    // Transparent — the margin around the card is per-pixel-alpha so the
    // desktop shows through (the window is `with_transparent(true)`). The soft
    // shadows below render onto that transparency as a floating-card shadow
    // rather than a black bevel.
    canvas.clear(Color::TRANSPARENT);

    let card = Rect::from_xywh(CARD_INSET, CARD_INSET, w - 2.0 * CARD_INSET, h - 2.0 * CARD_INSET);

    // No outer drop shadow — the card fills the window edge-to-edge, so there
    // is no margin to render one into. Edge definition comes from the inset
    // rose hairline rim in `draw_chrome_inner`.

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
    // Inset hairline rim: inset 0 0 0 1px rgba(229, 184, 197, 0.08)
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color(Color::from_argb((0.08 * 255.0) as u8, 229, 184, 197));
    let inset_rect = card.with_inset((0.5, 0.5));
    let inset_rrect = RRect::new_rect_xy(inset_rect, CARD_RADIUS - 0.5, CARD_RADIUS - 0.5);
    canvas.draw_rrect(inset_rrect, &rim);

    // Inner berry glow: ::after inset 0 0 80px -20px rgba(180, 100, 140, 0.15).
    // Static per window size — a sigma-40 mask-blur of an unchanging stroke —
    // so bake it once into a cached image and blit thereafter (see
    // `draw_inner_glow`), instead of re-blurring every frame.
    draw_inner_glow(canvas, w, h);
}

thread_local! {
    /// Cached inner-glow image, keyed by window size. The glow never changes
    /// for a given size, so a sigma-40 mask-blur per frame was pure waste.
    static GLOW_CACHE: RefCell<Option<(i32, i32, Image)>> = const { RefCell::new(None) };
}

/// Blit the cached inner berry glow, (re)baking it when the size changes.
fn draw_inner_glow(canvas: &Canvas, w: f32, h: f32) {
    let wi = w.round() as i32;
    let hi = h.round() as i32;
    if wi <= 0 || hi <= 0 {
        return;
    }

    let cached = GLOW_CACHE.with(|c| match c.borrow().as_ref() {
        Some((cw, ch, img)) if *cw == wi && *ch == hi => Some(img.clone()),
        _ => None,
    });

    let image = match cached {
        Some(img) => Some(img),
        None => {
            let built = build_inner_glow(canvas, w, h, wi, hi);
            if let Some(ref img) = built {
                GLOW_CACHE.with(|c| *c.borrow_mut() = Some((wi, hi, img.clone())));
            }
            built
        }
    };

    match image {
        Some(img) => {
            canvas.draw_image(&img, (0.0, 0.0), None);
        }
        // No offscreen available — draw straight (e.g. a non-GPU canvas).
        None => draw_inner_glow_direct(canvas, w, h),
    }
}

/// Bake the inner glow (clip included) into a transparent per-size sprite.
fn build_inner_glow(canvas: &Canvas, w: f32, h: f32, wi: i32, hi: i32) -> Option<Image> {
    let info = ImageInfo::new((wi, hi), ColorType::RGBA8888, AlphaType::Premul, None);
    let mut surface = canvas.new_surface(&info, None)?;
    let c = surface.canvas();
    c.clear(Color::TRANSPARENT);
    draw_inner_glow_direct(c, w, h);
    Some(surface.image_snapshot())
}

/// The actual glow draw: a blurred stroke clipped to the card rrect.
fn draw_inner_glow_direct(canvas: &Canvas, w: f32, h: f32) {
    let card = Rect::from_xywh(CARD_INSET, CARD_INSET, w - 2.0 * CARD_INSET, h - 2.0 * CARD_INSET);
    let card_rrect_ = RRect::new_rect_xy(card, CARD_RADIUS, CARD_RADIUS);
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

// Kept for a possible return of a window drop shadow (would need a margin —
// i.e. a small CARD_INSET — plus hit-testing that grabs the card edge).
#[allow(dead_code)]
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

