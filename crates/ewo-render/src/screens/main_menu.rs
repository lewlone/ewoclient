//! Main menu — the prototype's `screen-asym` layout. Pure typography for
//! step 7+; widgets (active-tab pill background, hover lift on menu items,
//! caret on hover) land in subsequent steps.
//!
//! Coordinate space: caller passes card-local dimensions (typically 1124×664
//! at default 1180×720 window). All positions are absolute in card-local px.
//!
//! CSS reference for layout:
//! ```css
//! .screen-asym  { padding: 72px 80px 60px; gap: 64px 72px; }
//! .asym-mark    { margin-top: 32px; gap: 18px; }
//! .asym-tagline { margin-top: 32px; }
//! .asym-footer  { font: 10px JetBrains Mono; letter-spacing: 0.4em; color: #6B555C; }
//! ```
//!
//! See `EwoClient · Velvet & Pearl prototype.htm` and `style/mainMenu.png`
//! for the visual ground truth.

use ewo_core::CubicBezier;
use skia_safe::{Canvas, Color, Color4f, Font, Paint, PaintStyle, Rect};

use crate::text::{self, FontStore, HoverGlowState};
use crate::widgets::VbtnState;

/// Draw a right-pointing chevron (">") from two strokes, with `tip_x` the
/// rightmost point. Used in place of "→"/"▸" glyphs — the bundled serif
/// display fonts don't carry those arrows, so they render as tofu boxes.
fn draw_chevron_right(
    canvas: &Canvas,
    tip_x: f32,
    cy: f32,
    half_h: f32,
    stroke: f32,
    color: Color,
    alpha: f32,
) {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(stroke);
    p.set_color(color);
    p.set_alpha_f(alpha);
    let back_x = tip_x - half_h * 0.72;
    canvas.draw_line((back_x, cy - half_h), (tip_x, cy), &p);
    canvas.draw_line((back_x, cy + half_h), (tip_x, cy), &p);
}

// CSS .screen-asym padding + margins
const PAD_LEFT: f32 = 80.0;
const PAD_TOP: f32 = 72.0;
const PAD_BOTTOM: f32 = 60.0;
const ASYM_MARK_MARGIN_TOP: f32 = 32.0;

// Token colors
const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const ACCENT_ROSE: Color = Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5);
const TAGLINE_GRAY: Color = Color::from_argb(0xFF, 0xA3, 0x8F, 0x95);

pub fn draw_main_menu(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    menu_states: &[VbtnState; 4],
    heading_hover: HoverGlowState,
    server: ServerWidgetView<'_>,
    enter_anim: f32,
) {
    // Smooth entrance — fade + slide-up when arriving at the main menu.
    // Alpha + translate only (never scale/blur text-bearing surfaces, per the
    // non-negotiables).
    let eased = CubicBezier::SILK.eval(enter_anim.clamp(0.0, 1.0));
    let layer = if eased < 0.999 {
        let dy = (1.0 - eased) * 14.0;
        let s = canvas.save_layer_alpha_f(Rect::from_xywh(0.0, 0.0, w, h), eased);
        canvas.translate((0.0, dy));
        Some(s)
    } else {
        None
    };
    draw_heading_block(canvas, fonts, time, heading_hover);
    draw_menu_items(canvas, fonts, w, h, menu_states);
    draw_server_widget(canvas, fonts, w, h, server);
    if let Some(s) = layer {
        canvas.restore_to_count(s);
    }
}

// ────────────────────────────────────────────────────────────────────────
// H6 — live network status widget (lower-left). Click joins the lobby.
// ────────────────────────────────────────────────────────────────────────

/// Render-side snapshot of the chickenedin network status. `main.rs` maps
/// `social::ServerStatus` into this so `ewo-render` stays ignorant of the
/// social / HTTP types (same pattern as `FriendRowView`).
#[derive(Clone, Copy, Default)]
pub struct ServerWidgetView<'a> {
    /// `None` until the first poll resolves — renders a "connecting…" state.
    pub data: Option<ServerWidgetData<'a>>,
    /// Cursor is over the widget (drives the hover brightening + JOIN hint).
    pub hovered: bool,
}

#[derive(Clone, Copy)]
pub struct ServerWidgetData<'a> {
    pub online: bool,
    pub online_count: u32,
    pub max_players: u32,
    pub tps: &'a str,
}

/// Card-local bounds of the network widget — also its click target. Sits in
/// the lower-left, above the footer.
pub fn server_widget_bounds(card_w: f32, card_h: f32) -> Rect {
    let width = 300.0_f32.min(card_w * 0.45);
    let height = 60.0;
    let bottom = card_h - PAD_BOTTOM - 36.0;
    Rect::from_xywh(PAD_LEFT, bottom - height, width, height)
}

fn draw_server_widget(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    view: ServerWidgetView<'_>,
) {
    const ACCENT_CHAMP: Color = Color::from_argb(0xFF, 0xE8, 0xD4, 0xA8);
    const ACCENT_EMBER: Color = Color::from_argb(0xFF, 0xC9, 0x6A, 0x7A);

    let bounds = server_widget_bounds(card_w, card_h);
    let rrect = skia_safe::RRect::new_rect_xy(bounds, 12.0, 12.0);
    let online = matches!(view.data, Some(d) if d.online);

    // Card fill — low-alpha rose tint, a touch brighter on hover.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    let fill_alpha = if view.hovered { 0.10 } else { 0.055 };
    fill.set_color4f(Color4f::new(0.71, 0.51, 0.62, fill_alpha), None);
    canvas.draw_rrect(rrect, &fill);

    // Hairline rim.
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color(ACCENT_ROSE);
    rim.set_alpha_f(if view.hovered { 0.32 } else { 0.16 });
    canvas.draw_rrect(rrect, &rim);

    let pad = 14.0;
    let left = bounds.left() + pad;

    // Status dot — champagne when online, ember when offline, dim before
    // the first poll resolves.
    let dot_cx = left + 3.5;
    let dot_cy = bounds.top() + 20.0;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color(if online { ACCENT_CHAMP } else { ACCENT_EMBER });
    dot.set_alpha_f(if view.data.is_some() { 1.0 } else { 0.4 });
    canvas.draw_circle((dot_cx, dot_cy), 3.5, &dot);

    // "CHICKENEDIN" eyebrow.
    let eyebrow_font = fonts.jetbrains_mono(9.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(TEXT_MAUVE);
    let (_, em) = eyebrow_font.metrics();
    let eyebrow_baseline = dot_cy + (-em.ascent + em.descent) / 2.0;
    draw_tracked(
        canvas,
        "CHICKENEDIN",
        left + 14.0,
        eyebrow_baseline,
        &eyebrow_font,
        &eyebrow_paint,
        0.18,
    );

    // JOIN hint on the right when online.
    if online {
        let join_font = fonts.jetbrains_mono(9.0);
        let mut join_paint = Paint::default();
        join_paint.set_anti_alias(true);
        join_paint.set_color(ACCENT_ROSE);
        join_paint.set_alpha_f(if view.hovered { 1.0 } else { 0.6 });
        // "JOIN" + a vector chevron (the "▸" glyph isn't in these fonts).
        let label = "JOIN";
        let lw = measure_tracked_width(&join_font, label, 0.18);
        let chevron_gap = 7.0;
        let chevron_tip = bounds.right() - pad;
        draw_tracked(
            canvas,
            label,
            chevron_tip - chevron_gap - lw,
            eyebrow_baseline,
            &join_font,
            &join_paint,
            0.18,
        );
        let chev_alpha = if view.hovered { 1.0 } else { 0.6 };
        draw_chevron_right(canvas, chevron_tip, eyebrow_baseline - 3.0, 3.5, 1.3, ACCENT_ROSE, chev_alpha);
    }

    // Status line.
    let status_font = fonts.jetbrains_mono(12.0);
    let mut status_paint = Paint::default();
    status_paint.set_anti_alias(true);
    status_paint.set_color(if online { TEXT_PEARL } else { TEXT_MAUVE });
    let (_, sm) = status_font.metrics();
    let status_baseline = bounds.top() + 42.0 + (-sm.ascent);
    let status_text = match view.data {
        None => "connecting…".to_string(),
        Some(d) if !d.online => "network offline".to_string(),
        Some(d) => format!(
            "{} / {} online · {} TPS",
            d.online_count, d.max_players, d.tps
        ),
    };
    canvas.draw_str(&status_text, (left, status_baseline), &status_font, &status_paint);
}

/// Card-local bounds for the four sidebar menu items, in rendered order.
/// Indices: 0=Instances, 1=Settings, 2=About, 3=Quit-to-desktop.
pub fn menu_item_bounds(card_w: f32, card_h: f32, fonts: &FontStore) -> [Rect; 4] {
    compute_menu_item_layout(card_w, card_h, fonts).item_rects
}

/// Card-local bounding rect of the "EwoClient" heading. Used by main.rs to
/// hover-test the cursor against the heading so the per-glyph glow fires
/// when the user mouses over the title.
pub fn heading_bounds(fonts: &FontStore) -> Rect {
    let heading_size = 94.0;
    let heading_font = fonts.fraunces_axes(heading_size, 50.0, 1.0, 300.0, None);
    let (_, hm) = heading_font.metrics();
    let mark_top = PAD_TOP + ASYM_MARK_MARGIN_TOP;
    let heading_baseline = mark_top + (-hm.ascent);
    // Approximate width — the actual breathing-text width oscillates with
    // the letter-spacing animation, so we use the maximum (base + amp)
    // so the hover region doesn't shrink mid-breath.
    let width = text::measure_tracked_em(&heading_font, "EwoClient", -0.035 + 0.02);
    let top = heading_baseline + hm.ascent;
    let bottom = heading_baseline + hm.descent;
    Rect::from_xywh(PAD_LEFT, top, width, bottom - top)
}

// ────────────────────────────────────────────────────────────────────────
// Heading block (left column): "EwoClient" + subtitle + tagline
// ────────────────────────────────────────────────────────────────────────

fn draw_heading_block(canvas: &Canvas, fonts: &FontStore, time: f32, heading_hover: HoverGlowState) {
    let mark_top = PAD_TOP + ASYM_MARK_MARGIN_TOP; // 104

    // Heading "EwoClient" — Fraunces 94, SOFT 50 / WONK 1 / wght 300.
    // Per-glyph layout with breathing letter-spacing:
    //   base = -0.035em (CSS `.launcher-title-xl` letter-spacing)
    //   amp  = +0.02em  (CSS `bt-breath` keyframe peak)
    //   period = 8s, smoothstepped triangle (≈ ease-in-out)
    let heading_size = 94.0;
    let heading_font = fonts.fraunces_axes(heading_size, 50.0, 1.0, 300.0, None);
    let (_, hm) = heading_font.metrics();
    let heading_baseline = mark_top + (-hm.ascent);

    let mut heading_paint = Paint::default();
    heading_paint.set_anti_alias(true);
    heading_paint.set_color(TEXT_PEARL);
    text::draw_breathing_glow(
        canvas,
        "EwoClient",
        (PAD_LEFT, heading_baseline),
        &heading_font,
        &heading_paint,
        -0.035,
        0.02,
        time,
        8.0,
        heading_hover,
    );
    let heading_bottom = heading_baseline + hm.descent;

    // Subtitle "V0.1 · VELVET BUILD · OFFLINE" — JetBrains Mono 10, tracked,
    // mauve, opacity 0.7. Sits below heading with the .asym-mark gap of 18px.
    let sub_size = 10.0;
    let sub_font = fonts.jetbrains_mono(sub_size);
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color(TEXT_MAUVE);
    sub_paint.set_alpha_f(0.7);
    let (_, sm) = sub_font.metrics();
    let sub_top = heading_bottom + 18.0;
    let sub_baseline = sub_top + (-sm.ascent);
    draw_tracked(
        canvas,
        "V0.1 · VELVET BUILD · OFFLINE",
        PAD_LEFT,
        sub_baseline,
        &sub_font,
        &sub_paint,
        0.35,
    );
    let sub_bottom = sub_baseline + sm.descent;

    // Tagline "— a place to wait in the dark —" — Newsreader Italic 18, color
    // #A38F95, letter-spacing 0.04em. CSS margin-top: 32 from the asym-mark
    // flex parent (overrides the gap).
    let tag_size = 18.0;
    let tag_font = newsreader_italic_axes(fonts, tag_size, 400.0);
    let mut tag_paint = Paint::default();
    tag_paint.set_anti_alias(true);
    tag_paint.set_color(TAGLINE_GRAY);
    let (_, tm) = tag_font.metrics();
    let tag_top = sub_bottom + 32.0;
    let tag_baseline = tag_top + (-tm.ascent);
    draw_tracked(
        canvas,
        "— a place to wait in the dark —",
        PAD_LEFT,
        tag_baseline,
        &tag_font,
        &tag_paint,
        0.04,
    );
}

// ────────────────────────────────────────────────────────────────────────
// Right column — four menu items
// ────────────────────────────────────────────────────────────────────────

struct MenuItemDef {
    label: &'static str,
    sub: &'static str,
    muted: bool,
}

const MENU_ITEMS: [MenuItemDef; 4] = [
    MenuItemDef { label: "Instances", sub: "LAST PLAYED · VELVET HOURS", muted: false },
    MenuItemDef { label: "Settings", sub: "GRAPHICS · AUDIO · PATHS", muted: false },
    MenuItemDef { label: "About", sub: "VELVET BUILD · V0.1", muted: false },
    MenuItemDef { label: "Quit to desktop", sub: "CLOSE THE CURTAIN", muted: true },
];

const LABEL_SIZE: f32 = 32.0;
const MUTED_SIZE: f32 = 22.0;
const SUB_SIZE: f32 = 10.0;
const ITEM_PAD_Y: f32 = 18.0;
const LABEL_TO_SUB_GAP: f32 = 4.0;

struct MenuLayout {
    item_rects: [Rect; 4],
    menu_left: f32,
    menu_right: f32,
}

fn compute_menu_item_layout(card_w: f32, card_h: f32, fonts: &FontStore) -> MenuLayout {
    let label_font = fonts.fraunces_axes(LABEL_SIZE, 50.0, 1.0, 300.0, None);
    let muted_font = newsreader_italic_axes(fonts, MUTED_SIZE, 300.0);
    let sub_font = fonts.jetbrains_mono(SUB_SIZE);
    let (_, lm) = label_font.metrics();
    let (_, mm) = muted_font.metrics();
    let (_, sm) = sub_font.metrics();

    let label_visual_h = -lm.ascent + lm.descent;
    let muted_visual_h = -mm.ascent + mm.descent;
    let sub_visual_h = -sm.ascent + sm.descent;

    let item_heights: [f32; 4] = std::array::from_fn(|i| {
        let lh = if MENU_ITEMS[i].muted { muted_visual_h } else { label_visual_h };
        2.0 * ITEM_PAD_Y + lh + LABEL_TO_SUB_GAP + sub_visual_h
    });

    let row1_bottom = card_h - PAD_BOTTOM - 64.0 - 14.0;
    let menu_bottom = row1_bottom - 12.0;
    let total_h: f32 = item_heights.iter().sum::<f32>() + 2.0 * (MENU_ITEMS.len() as f32 - 1.0);
    let menu_top = menu_bottom - total_h;
    let menu_max_width = 480.0_f32.min(card_w - PAD_LEFT - 80.0);
    let menu_right = card_w - PAD_LEFT;
    let menu_left = menu_right - menu_max_width;

    let mut item_rects: [Rect; 4] = [Rect::default(); 4];
    let mut y = menu_top;
    for i in 0..4 {
        let h_item = item_heights[i];
        item_rects[i] = Rect::from_xywh(menu_left, y, menu_right - menu_left, h_item);
        y += h_item + 2.0;
    }
    MenuLayout { item_rects, menu_left, menu_right }
}

fn draw_menu_items(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    states: &[VbtnState; 4],
) {
    let label_font = fonts.fraunces_axes(LABEL_SIZE, 50.0, 1.0, 300.0, None);
    let muted_font = newsreader_italic_axes(fonts, MUTED_SIZE, 300.0);
    let sub_font = fonts.jetbrains_mono(SUB_SIZE);
    let (_, lm) = label_font.metrics();
    let (_, mm) = muted_font.metrics();
    let (_, sm) = sub_font.metrics();

    let label_visual_h = -lm.ascent + lm.descent;
    let muted_visual_h = -mm.ascent + mm.descent;

    let layout = compute_menu_item_layout(w, h, fonts);
    let menu_left = layout.menu_left;
    let menu_right = layout.menu_right;

    for (i, item) in MENU_ITEMS.iter().enumerate() {
        let item_rect = layout.item_rects[i];
        let state = &states[i];
        // Silk-eased hover progress (0..1).
        let hover = CubicBezier::SILK.eval(state.hover_anim.clamp(0.0, 1.0));
        let pad_left = 16.0 * hover; // CSS `:hover { padding-left: 16px }`

        let label_top = item_rect.top + ITEM_PAD_Y;
        let baseline_x = menu_left + pad_left;

        // Accent rule on the left — width 0 → 14, opacity 0 → 1 at silk.
        // CSS `.menu-item-rule`: top: 34px (vertically aligned to label midline),
        //   gradient #E5B8C5 → #C9A5D4, box-shadow 0 0 8 rgba(229,184,197,0.6).
        if hover > 0.001 {
            let rule_w = 14.0 * hover;
            let rule_y = label_top
                + if item.muted {
                    muted_visual_h * 0.5
                } else {
                    label_visual_h * 0.5
                };
            let rule_alpha = hover; // opacity 0 → 1
            let mut rule = Paint::default();
            rule.set_anti_alias(true);
            rule.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, rule_alpha),
                None,
            );
            // Gradient effect approximated with a single color; the prototype's
            // gradient is short and the color shift is subtle at this width.
            canvas.draw_rect(
                Rect::from_xywh(menu_left - 16.0 + pad_left, rule_y, rule_w, 1.0),
                &rule,
            );
            // Subtle glow: a slightly larger softer rect underneath
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.15 * rule_alpha),
                None,
            );
            canvas.draw_rect(
                Rect::from_xywh(menu_left - 16.0 + pad_left, rule_y - 1.0, rule_w, 3.0),
                &glow,
            );
        }

        // Label — with a hover-glow behind it (the hero-title treatment) and
        // a slight warm-white brighten on hover.
        if item.muted {
            let baseline = label_top + (-mm.ascent);
            text::draw_glow_str(canvas, item.label, (baseline_x, baseline), &muted_font, hover * 0.85);
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(TEXT_MAUVE);
            canvas.draw_str(item.label, (baseline_x, baseline), &muted_font, &paint);
        } else {
            let baseline = label_top + (-lm.ascent);
            text::draw_glow_str(canvas, item.label, (baseline_x, baseline), &label_font, hover * 0.85);
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            // Pearl → warm-white (#FFF0F4) as hover ramps.
            paint.set_color4f(
                Color4f::new(
                    (0xF4 as f32 + (0xFF - 0xF4) as f32 * hover) / 255.0,
                    (0xE8 as f32 + (0xF0 - 0xE8) as f32 * hover) / 255.0,
                    (0xEA as f32 + (0xF4 - 0xEA) as f32 * hover) / 255.0,
                    1.0,
                ),
                None,
            );
            canvas.draw_str(item.label, (baseline_x, baseline), &label_font, &paint);
        }

        // Sub-label
        let label_bottom = label_top
            + if item.muted {
                muted_visual_h
            } else {
                label_visual_h
            };
        let sub_top = label_bottom + LABEL_TO_SUB_GAP;
        let sub_baseline = sub_top + (-sm.ascent);
        let mut sub_paint = Paint::default();
        sub_paint.set_anti_alias(true);
        sub_paint.set_color(if item.muted { TEXT_MAUVE_DEEP } else { TEXT_MAUVE });
        draw_tracked(canvas, item.sub, baseline_x, sub_baseline, &sub_font, &sub_paint, 0.22);

        // Caret on the right — opacity 0 → 1, transform translateX(-6) → 0.
        // Drawn as a vector chevron rather than a "→" glyph: the serif
        // display fonts don't carry the arrow and render it as a tofu box.
        if hover > 0.001 {
            let color = if item.muted { TEXT_MAUVE } else { ACCENT_ROSE };
            let translate_x = -6.0 * (1.0 - hover);
            let cy = item_rect.top + item_rect.height() * 0.5;
            let tip_x = menu_right - 5.0 + translate_x;
            draw_chevron_right(canvas, tip_x, cy, 6.0, 1.8, color, hover);
        }

        // Hairline divider at the bottom of each item except the last.
        if i < MENU_ITEMS.len() - 1 {
            let div_y = item_rect.bottom;
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
                None,
            );
            div.set_stroke_width(1.0);
            div.set_style(PaintStyle::Stroke);
            canvas.draw_line((menu_left, div_y), (menu_right, div_y), &div);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────

/// Build a Newsreader italic Font with explicit axes (opsz tracks size, wght
/// configurable). Falls back to the default cut if axes aren't applicable.
fn newsreader_italic_axes(fonts: &FontStore, size: f32, weight: f32) -> Font {
    use skia_safe::font_arguments::variation_position::Coordinate;
    use skia_safe::font_arguments::VariationPosition;
    use skia_safe::{FontArguments, FourByteTag};

    let coords = [
        Coordinate { axis: FourByteTag::from_chars('o', 'p', 's', 'z'), value: size },
        Coordinate { axis: FourByteTag::from_chars('w', 'g', 'h', 't'), value: weight },
    ];
    let pos = VariationPosition { coordinates: &coords };
    let args = FontArguments::new().set_variation_design_position(pos);
    let tf = fonts
        .newsreader_italic_typeface()
        .clone_with_arguments(&args)
        .unwrap_or_else(|| fonts.newsreader_italic_typeface().clone());
    let mut f = Font::new(tf, size);
    f.set_subpixel(true);
    f
}

/// Thin wrappers around `text::draw_tracked_em` / `text::measure_tracked_em`
/// that take an `(x, baseline_y)` pair rather than a tuple — keeps existing
/// call sites in this module readable while delegating to the canonical
/// helpers in `text.rs`.
fn draw_tracked(
    canvas: &Canvas,
    s: &str,
    x: f32,
    baseline_y: f32,
    font: &Font,
    paint: &Paint,
    tracking_em: f32,
) {
    text::draw_tracked_em(canvas, s, (x, baseline_y), font, paint, tracking_em);
}

fn measure_tracked_width(font: &Font, s: &str, tracking_em: f32) -> f32 {
    text::measure_tracked_em(font, s, tracking_em)
}
