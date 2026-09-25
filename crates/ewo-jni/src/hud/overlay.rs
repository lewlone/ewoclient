use super::*;

/// The overlay's top-centre view-tab strip: the whole pill + a rect per tab.
/// Fixed-width tabs so the renderer and the hit-tester agree without fonts.
pub(super) fn tab_layout(w: f32) -> (Rect, Vec<Rect>) {
    // One slot per `OverlayView::ALL` entry — derived from the array length so
    // adding a view (e.g. FRIENDS) can't desync the strip from the click
    // hit-test, and so no tab gets silently clipped off the end.
    let n = OverlayView::ALL.len();
    const TAB_W: f32 = 102.0; // narrowed for the 8-tab strip.
    const TAB_H: f32 = 34.0;
    const TAB_Y: f32 = 18.0;
    let strip_w = TAB_W * n as f32;
    let strip_x = (w - strip_w) * 0.5;
    let pill = Rect::from_xywh(strip_x, TAB_Y, strip_w, TAB_H);
    let tabs = (0..n)
        .map(|i| Rect::from_xywh(strip_x + i as f32 * TAB_W, TAB_Y, TAB_W, TAB_H))
        .collect();
    (pill, tabs)
}

/// Draw the view-tab strip; the active view's tab is lit with the design's
/// rose-berry vertical gradient + glow.
pub(super) fn draw_tab_strip(canvas: &Canvas, view: OverlayView, fonts: &FontStore, w: f32) {
    let (pill, tabs) = tab_layout(w);

    // Pill container — flat dark wine, hairline pearl border. Matches the
    // redesign's `.tabs` (`background:rgba(10,0,8,.55); border:1px solid hair`).
    let pill_r = pill.height() * 0.5;
    let pill_rr = RRect::new_rect_xy(pill, pill_r, pill_r);
    let mut pill_fill = Paint::default();
    pill_fill.set_anti_alias(true);
    pill_fill.set_color4f(rgba(WINE, 0.55), None);
    canvas.draw_rrect(pill_rr, &pill_fill);
    let mut pill_border = Paint::default();
    pill_border.set_anti_alias(true);
    pill_border.set_style(PaintStyle::Stroke);
    pill_border.set_stroke_width(1.0);
    pill_border.set_color4f(rgba(PEARL, 0.08), None);
    canvas.draw_rrect(pill_rr, &pill_border);

    let font = fonts.jetbrains_mono(11.0);
    let tracking = 0.22; // CSS `.tab { letter-spacing: .22em }`.
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };

    for (i, &tab) in tabs.iter().enumerate() {
        let v = OverlayView::ALL[i];
        let active = v == view;
        if active {
            // The redesign's `.tab.active`: 180° berry → deep-wine vertical
            // gradient + inset 1px pearl top highlight + outer rose glow.
            let inset = Rect::from_xywh(
                tab.left + 4.0,
                tab.top + 4.0,
                tab.width() - 8.0,
                tab.height() - 8.0,
            );
            let inset_r = inset.height() * 0.5;
            let inset_rr = RRect::new_rect_xy(inset, inset_r, inset_r);

            // Outer rose glow (CSS `0 0 22px -4px rgba(180,116,145,.55)`).
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(rgba(BERRY, 0.4), None);
            glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 11.0, false));
            canvas.draw_rrect(inset_rr, &glow);

            // Vertical berry→deep-wine fill.
            let mut fill = Paint::default();
            fill.set_anti_alias(true);
            if let Some(shader) = gradient_shader::linear(
                (
                    Point::new(inset.left, inset.top),
                    Point::new(inset.left, inset.bottom),
                ),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[
                        rgba(BERRY, 0.62),
                        Color4f::new(
                            0x7F as f32 / 255.0,
                            0x40 as f32 / 255.0,
                            0x5A as f32 / 255.0,
                            0.72,
                        ),
                    ],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill.set_shader(shader);
            }
            canvas.draw_rrect(inset_rr, &fill);

            // Inset top-edge highlight (CSS `inset 0 1px 0 rgba(255,255,255,.18)`).
            canvas.save();
            let top_clip = Rect::from_xywh(inset.left, inset.top, inset.width(), 2.0);
            canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
            let inset_hi = Rect::new(
                inset.left + 0.5,
                inset.top + 0.5,
                inset.right - 0.5,
                inset.bottom - 0.5,
            );
            let inset_hi_r = (inset_r - 0.5).max(0.0);
            let mut top_paint = Paint::default();
            top_paint.set_anti_alias(true);
            top_paint.set_style(PaintStyle::Stroke);
            top_paint.set_stroke_width(1.0);
            top_paint.set_color4f(rgba(PEARL, 0.22), None);
            canvas.draw_rrect(
                RRect::new_rect_xy(inset_hi, inset_hi_r, inset_hi_r),
                &top_paint,
            );
            canvas.restore();
        }

        let label = v.title();
        let label_w = measure_tracked_em(&font, label, tracking);
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color4f(
            if active { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) },
            None,
        );
        draw_tracked_em(
            canvas,
            label,
            (
                tab.left + (tab.width() - label_w) * 0.5,
                tab.top + tab.height() * 0.5 + cap * 0.5,
            ),
            &font,
            &paint,
            tracking,
        );
    }
}

/// The Settings-view panel rect, its client-profile chips, and the 4
/// paint-rate selector buttons. Fixed-size so renderer + hit-tester agree.
pub(super) fn settings_layout(w: f32, h: f32, profile_count: usize) -> (Rect, Vec<Rect>, [Rect; 4], Rect) {
    const CHIP_W: f32 = 112.0;
    const CHIP_H: f32 = 32.0;
    const GAP: f32 = 8.0;
    let chip_rows = (profile_count.max(1) as f32 / 4.0).ceil();

    let pw = 484.0;
    // +96 for the glass-strength row (label, hint, track).
    let ph = 262.0 + 96.0 + chip_rows * (CHIP_H + GAP);
    let px = (w - pw) * 0.5;
    let py = (h - ph) * 0.5;
    let panel = Rect::from_xywh(px, py, pw, ph);
    let left = px + 32.0;

    // Client-profile chips — four per row, under the "CLIENT PROFILE" label.
    let chips_top = py + 122.0;
    let mut chips = Vec::with_capacity(profile_count);
    for i in 0..profile_count {
        let col = (i % 4) as f32;
        let row = (i / 4) as f32;
        chips.push(Rect::from_xywh(
            left + col * (CHIP_W + GAP),
            chips_top + row * (CHIP_H + GAP),
            CHIP_W,
            CHIP_H,
        ));
    }

    // Paint-rate buttons — below the profile section + its labels.
    const BTN_W: f32 = 92.0;
    const BTN_H: f32 = 38.0;
    const BTN_GAP: f32 = 10.0;
    let buttons_top = chips_top + chip_rows * (CHIP_H + GAP) + 64.0;
    let mut buttons = [empty_rect(); 4];
    for (i, slot) in buttons.iter_mut().enumerate() {
        *slot = Rect::from_xywh(left + i as f32 * (BTN_W + BTN_GAP), buttons_top, BTN_W, BTN_H);
    }

    // Glass-strength track. Generous height so it is easy to grab; the visible
    // track is drawn thin, centred inside this.
    let glass = Rect::from_xywh(left, buttons_top + BTN_H + 62.0, pw - 64.0, 20.0);

    (panel, chips, buttons, glass)
}

/// Map a cursor x to a glass-strength value on `track`.
pub(super) fn glass_value_at(track: Rect, x: f32) -> f32 {
    let frac = ((x - track.left) / track.width().max(1.0)).clamp(0.0, 1.0);
    let raw = GLASS_MIN + (GLASS_MAX - GLASS_MIN) * frac;
    // Snap to 0.05 so the persisted value is tidy and the readout doesn't
    // jitter in the last decimal while dragging.
    (raw / 0.05).round() * 0.05
}

/// Quick-edit overlay — the HUD editor's affordances without its furniture.
///
/// Drawn over a vanilla screen while the modifier is held, so the widgets the
/// player can grab are visible without the EwoClient overlay taking over. No
/// scrim, no panel, no tab strip: the inventory behind stays fully readable.
pub(super) fn draw_quick_edit(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    // Snap guides, same as the editor tab.
    let mut guide = Paint::default();
    guide.set_anti_alias(false);
    guide.set_style(PaintStyle::Stroke);
    guide.set_stroke_width(1.0);
    guide.set_color4f(rgba(ROSE, 0.55), None);
    if let Some(sx) = editor.snap_x {
        canvas.draw_line((sx, 0.0), (sx, h), &guide);
    }
    if let Some(sy) = editor.snap_y {
        canvas.draw_line((0.0, sy), (w, sy), &guide);
    }

    let active = editor.active_widget();
    for id in WidgetId::ALL {
        let b = editor.bounds[id.index()];
        if b.width() <= 0.0 {
            continue;
        }
        let wl = editor.layout.get(id);
        let lit = Some(id) == active || Some(id) == editor.selected;
        draw_widget_outline(
            canvas,
            b,
            lit,
            fonts,
            id.title(),
            wl.anchor,
            wl.scale,
            Some(id) == editor.selected,
        );
    }

    // A single quiet line, bottom-centre — enough to explain the mode to
    // someone who discovered it by accident, small enough to ignore.
    let hint_font = fonts.jetbrains_mono(11.0);
    let hint = "QUICK EDIT  ·  DRAG TO MOVE  ·  CORNER TO RESIZE";
    let hint_w = measure_tracked_em(&hint_font, hint, 0.14);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.75), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    draw_tracked_em(
        canvas,
        hint,
        ((w - hint_w) * 0.5, h - 28.0),
        &hint_font,
        &shadow,
        0.14,
    );
    let mut hint_paint = Paint::default();
    hint_paint.set_anti_alias(true);
    hint_paint.set_color4f(rgba(ROSE, 1.0), None);
    draw_tracked_em(
        canvas,
        hint,
        ((w - hint_w) * 0.5, h - 28.0),
        &hint_font,
        &hint_paint,
        0.14,
    );
}

/// One option button in the paint-rate selector.
pub(super) fn draw_settings_button(canvas: &Canvas, rect: Rect, label: &str, active: bool, fonts: &FontStore) {
    let rrect = RRect::new_rect_xy(rect, 9.0, 9.0);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if active { rgba(ROSE, 0.55) } else { rgba(WINE, 0.8) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        if active { rgba(ROSE, 0.85) } else { rgba(ROSE, 0.18) },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    let font = fonts.jetbrains_mono(13.0);
    let tracking = 0.10;
    let label_w = measure_tracked_em(&font, label, tracking);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(
        if active { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) },
        None,
    );
    draw_tracked_em(
        canvas,
        label,
        (
            rect.left + (rect.width() - label_w) * 0.5,
            rect.top + rect.height() * 0.5 + cap * 0.5,
        ),
        &font,
        &text,
        tracking,
    );
}

/// The Settings view — a client-profile picker + the HUD paint-rate cap.
pub(super) fn draw_settings(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let (panel, chips, buttons, glass) = settings_layout(w, h, editor.profiles.len());
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 32.0;

    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "SETTINGS",
        (left, panel.top + 40.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("HUD preferences", (left, panel.top + 78.0), &title_font, &title);

    let label_font = fonts.jetbrains_mono(10.0);

    // Client-profile picker — chips, the active one lit.
    let mut prof_label = Paint::default();
    prof_label.set_anti_alias(true);
    prof_label.set_color4f(rgba(MAUVE, 1.0), None);
    let prof_label_y = chips
        .first()
        .map(|c| c.top - 14.0)
        .unwrap_or(panel.top + 108.0);
    draw_tracked_em(
        canvas,
        "CLIENT PROFILE",
        (left, prof_label_y),
        &label_font,
        &prof_label,
        0.18,
    );
    for (chip, name) in chips.iter().zip(&editor.profiles) {
        draw_settings_button(canvas, *chip, name, *name == editor.active_profile, fonts);
    }

    // Paint-rate section — positioned relative to the buttons.
    let mut pr_label = Paint::default();
    pr_label.set_anti_alias(true);
    pr_label.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "PAINT RATE  ·  FPS CAP",
        (left, buttons[0].top - 46.0),
        &label_font,
        &pr_label,
        0.18,
    );

    let body_font = fonts.newsreader(13.0);
    let mut body = Paint::default();
    body.set_anti_alias(true);
    body.set_color4f(rgba(MAUVE, 0.85), None);
    canvas.draw_str(
        "How often the HUD repaints. A lower cap frees GPU for the game.",
        (left, buttons[0].top - 22.0),
        &body_font,
        &body,
    );

    let current = editor.paint_rate();
    for (i, &btn) in buttons.iter().enumerate() {
        let rate = crate::HudPaintRate::ALL[i];
        draw_settings_button(canvas, btn, rate.label(), rate == current, fonts);
    }

    // ── Liquid-glass strength ────────────────────────────────────────────
    let strength = editor.glass_strength();
    let mut gl_label = Paint::default();
    gl_label.set_anti_alias(true);
    gl_label.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "GLASS  ·  REFRACTION",
        (left, glass.top - 40.0),
        &label_font,
        &gl_label,
        0.18,
    );
    canvas.draw_str(
        if strength <= 0.0 {
            "Off — flat plates. Cheapest, and the pre-glass look."
        } else {
            "How much the widget edges bend the world behind them."
        },
        (left, glass.top - 18.0),
        &body_font,
        &body,
    );

    // Value readout, right-aligned against the track's end.
    let val_font = fonts.jetbrains_mono(11.0);
    let val = if strength <= 0.0 {
        "OFF".to_string()
    } else {
        format!("{strength:.2}×")
    };
    let val_w = measure_tracked_em(&val_font, &val, 0.14);
    let mut val_paint = Paint::default();
    val_paint.set_anti_alias(true);
    val_paint.set_color4f(rgba(ROSE, 1.0), None);
    draw_tracked_em(
        canvas,
        &val,
        (glass.right - val_w, glass.top - 18.0),
        &val_font,
        &val_paint,
        0.14,
    );

    // Track + fill + handle. Deliberately the same visual language as the
    // launcher's vslider (2px pill, rose fill, pearl-cored handle) rather than
    // yet another bespoke slider — the in-game surface already has five.
    let cy = glass.center_y();
    let track = Rect::from_xywh(glass.left, cy - 1.0, glass.width(), 2.0);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(ROSE, 0.16), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, 1.0, 1.0), &track_paint);

    let frac = ((strength - GLASS_MIN) / (GLASS_MAX - GLASS_MIN)).clamp(0.0, 1.0);
    if frac > 0.0 {
        let mut fill = track;
        fill.right = fill.left + track.width() * frac;
        let mut fill_paint = Paint::default();
        fill_paint.set_anti_alias(true);
        fill_paint.set_color4f(rgba(ROSE, 0.95), None);
        canvas.draw_rrect(RRect::new_rect_xy(fill, 1.0, 1.0), &fill_paint);
    }

    let hx = glass.left + track.width() * frac;
    let mut halo = Paint::default();
    halo.set_anti_alias(true);
    halo.set_color4f(rgba(ROSE, 0.45), None);
    halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
    canvas.draw_circle((hx, cy), 8.0, &halo);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_circle((hx, cy), 5.0, &knob);
}

// ────────────────────────────────────────────────────────────────────────
// Home view — the session overview + quick toggles.
// ────────────────────────────────────────────────────────────────────────

/// The HOME-view panel, the 3D-skin viewport, 5 stat cards, the media-
/// controller card, and the quick-toggle area (a Rect — the chips themselves
/// flow at render time because their widths depend on the per-label text
/// measurement; the renderer caches the resulting rects in
/// `Editor::home_toggle_bounds` for the press handler).
pub(super) fn home_layout(w: f32, h: f32) -> (Rect, Rect, [Rect; 5], Rect, Rect) {
    let pw = 780.0; // grew again for chip flow + breathing room.
    let ph = 720.0;
    let panel = Rect::from_xywh((w - pw) * 0.5, (h - ph) * 0.5, pw, ph);
    let gap = 12.0;

    // Left column — the 3D skin viewer. Sized to look like a *small* portrait
    // (not the entire panel height) — old layout had the model dominate.
    let skin_w = 150.0;
    let skin_h = 360.0;
    let skin = Rect::from_xywh(
        panel.left + 32.0,
        panel.top + 124.0,
        skin_w,
        skin_h,
    );

    // Right column — stat cards (a 2-wide grid, the last card full width).
    let rx = skin.right + 28.0;
    let rw = panel.right - 36.0 - rx;
    let card_h = 60.0;
    let card_w = (rw - gap) / 2.0;
    let stats_top = panel.top + 124.0;
    let step = card_h + gap;
    let stats = [
        Rect::from_xywh(rx, stats_top, card_w, card_h),
        Rect::from_xywh(rx + card_w + gap, stats_top, card_w, card_h),
        Rect::from_xywh(rx, stats_top + step, card_w, card_h),
        Rect::from_xywh(rx + card_w + gap, stats_top + step, card_w, card_h),
        Rect::from_xywh(rx, stats_top + 2.0 * step, rw, card_h),
    ];

    // Media controller large card — full-width, ~120px tall. Sits below the
    // account row (which draws after the stat grid in `draw_home`); the
    // vertical gap below stats was bumped from 76 → 96 to give the account
    // line breathing room.
    let media_top = stats_top + 2.0 * step + card_h + 96.0;
    let media_h = 120.0;
    let media = Rect::from_xywh(rx, media_top, rw, media_h);

    // Quick-toggle area — chips flow inside this rect; bumped gap from 22 → 32
    // so the eyebrow + chip rows aren't smushed against the media card.
    let toggle_area = Rect::from_xywh(rx, media.bottom + 32.0, rw, 138.0);

    (panel, skin, stats, media, toggle_area)
}

/// Format session seconds as `m:ss`, or `h:mm:ss` past an hour.
pub(super) fn fmt_playtime(secs: i32) -> String {
    let s = secs.max(0);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{}:{:02}", m, sec)
    }
}

/// `%APPDATA%/EwoClient/profiles.toml`.
pub(super) fn profiles_toml_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("EwoClient").join("profiles.toml"))
}

/// Read `profiles.toml` — `(active, all profile names)`. `None` if the file
/// is missing or unreadable. The launcher owns this file; the in-game side
/// reads it and rewrites only the `active` pointer.
pub(super) fn read_profiles() -> Option<(String, Vec<String>)> {
    let text = std::fs::read_to_string(profiles_toml_path()?).ok()?;
    let mut active = String::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("active") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                active = val.trim().trim_matches('"').to_string();
                break;
            }
        }
    }
    // Every quoted string after the `profiles` key is a profile name —
    // robust to inline or wrapped TOML arrays.
    let mut all: Vec<String> = Vec::new();
    let after = text
        .find("\nprofiles")
        .map(|i| &text[i..])
        .or_else(|| text.starts_with("profiles").then_some(text.as_str()));
    if let Some(mut rest) = after {
        while let Some(q1) = rest.find('"') {
            let tail = &rest[q1 + 1..];
            let Some(q2) = tail.find('"') else { break };
            all.push(tail[..q2].to_string());
            rest = &tail[q2 + 1..];
        }
    }
    if active.is_empty() && all.is_empty() {
        return None;
    }
    if active.is_empty() {
        active = all.first().cloned().unwrap_or_else(|| "Default".to_string());
    }
    if all.is_empty() {
        all.push(active.clone());
    }
    Some((active, all))
}

/// The active client-profile name, or `None` if `profiles.toml` is absent.
pub(crate) fn read_active_profile() -> Option<String> {
    read_profiles().map(|(active, _)| active)
}

/// Rewrite `profiles.toml` with a new active profile. The file holds only
/// `active` + `profiles`, so a full rewrite is total and the launcher
/// re-reads it cleanly.
pub(super) fn write_profiles(active: &str, all: &[String]) {
    let Some(path) = profiles_toml_path() else {
        return;
    };
    let mut s = format!("active = \"{active}\"\nprofiles = [");
    for (i, p) in all.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('"');
        s.push_str(p);
        s.push('"');
    }
    s.push_str("]\n");
    let _ = std::fs::write(&path, s);
}

/// One stat card on the HOME view — a chip with a mono label and a value,
/// plus the design's `.card.hairglow` subtle pearl-tint chrome:
/// `inset 0 1px 0 rgba(244,232,234,.06)` top highlight + `0 0 0 1px rgba(229,184,197,.04)`
/// rose tint ring. The lifts cards off the panel without screaming for attention.
pub(super) fn draw_stat_card(canvas: &Canvas, rect: Rect, label: &str, value: &str, fonts: &FontStore) {
    let radius = 12.0;
    draw_chip(canvas, rect, radius);

    // `.card.hairglow` — a 1px rose tint ring just outside the chip, and an
    // inset pearl 1px highlight along the top edge.
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let mut tint = Paint::default();
    tint.set_anti_alias(true);
    tint.set_style(PaintStyle::Stroke);
    tint.set_stroke_width(1.0);
    tint.set_color4f(rgba(ROSE, 0.10), None);
    canvas.draw_rrect(rrect, &tint);

    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, radius - 0.5, radius - 0.5), &top);
    canvas.restore();

    let left = rect.left + 14.0;

    let label_font = fonts.jetbrains_mono(9.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, label, (left, rect.top + 22.0), &label_font, &label_paint, 0.22);

    let value_font = fonts.fraunces_axes(20.0, 36.0, 0.0, 560.0, None);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(value, (left, rect.top + 48.0), &value_font, &value_paint);
}

/// One quick-toggle pill — re-skin of the design's `.qpill`. A wine pill with
/// an LED dot + label; when on, the pill takes a vertical berry-wine gradient
/// fill + a brighter rose border + a glowing rose LED. Hover shading is added
/// by the outer hit-test (HOME doesn't currently track hover).
pub(super) fn draw_toggle_chip(canvas: &Canvas, rect: Rect, label: &str, on: bool, fonts: &FontStore) {
    let r = rect.height() * 0.5;
    let rrect = RRect::new_rect_xy(rect, r, r);

    // Fill — flat wine when off, top-to-bottom berry-wine gradient when on
    // (CSS `linear-gradient(180deg, rgba(180,116,145,.35), rgba(127,64,90,.35))`).
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    if on {
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.left, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(BERRY, 0.45),
                    Color4f::new(0x7F as f32 / 255.0, 0x40 as f32 / 255.0, 0x5A as f32 / 255.0, 0.45),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            bg.set_shader(shader);
        }
    } else {
        bg.set_color4f(rgba(WINE, 0.55), None);
    }
    canvas.draw_rrect(rrect, &bg);

    // Border — rose 0.32 when on, hairline pearl 0.10 when off.
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(ROSE, 0.34) } else { rgba(PEARL, 0.10) }, None);
    canvas.draw_rrect(rrect, &border);

    let cy = (rect.top + rect.bottom) * 0.5;
    let dot_x = rect.left + 15.0;

    // LED dot — when on, a bright rose disc with a blurred rose halo behind it.
    if on {
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(rgba(ROSE, 0.55), None);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
        canvas.draw_circle((dot_x, cy), 5.0, &halo);
    }
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(
        if on {
            rgba(ROSE, 1.0)
        } else {
            Color4f::new(0x43 as f32 / 255.0, 0x24 as f32 / 255.0, 0x2E as f32 / 255.0, 1.0)
        },
        None,
    );
    canvas.draw_circle((dot_x, cy), 3.0, &dot);

    let font = fonts.jetbrains_mono(9.0);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) }, None);
    draw_tracked_em(canvas, label, (dot_x + 12.0, cy + cap * 0.5), &font, &text, 0.22);
}

/// The HOME / overview view — a rotatable 3D skin, session stats, the
/// account + profile, and quick per-HUD-widget visibility toggles.
pub(super) fn draw_home(canvas: &Canvas, editor: &mut Editor, data: &HudData, fonts: &FontStore, w: f32, h: f32) {
    let (panel, skin_rect, stats, media_rect, toggle_area) = home_layout(w, h);
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 28.0;

    // ── Crumb breadcrumb — glowing rose dot + tracked mono eyebrow ─────────
    // The redesign's `.crumb` pattern (`<span class="dot"></span> Home · Overview`).
    let crumb_baseline = panel.top + 40.0;
    let dot_x = left + 4.0;
    let dot_y = crumb_baseline - 4.0;
    let mut dot_halo = Paint::default();
    dot_halo.set_anti_alias(true);
    dot_halo.set_color4f(rgba(ROSE, 0.65), None);
    dot_halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
    canvas.draw_circle((dot_x, dot_y), 4.0, &dot_halo);
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(rgba(ROSE, 1.0), None);
    canvas.draw_circle((dot_x, dot_y), 2.0, &dot);

    let crumb_font = fonts.jetbrains_mono(10.5);
    let mut crumb = Paint::default();
    crumb.set_anti_alias(true);
    crumb.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "HOME  ·  OVERVIEW",
        (dot_x + 14.0, crumb_baseline),
        &crumb_font,
        &crumb,
        0.22,
    );

    // ── Title — "Welcome back, <player>" with the name in rose italic ──────
    let player = data.player_name();
    let display_name = if player.is_empty() { editor.active_profile.clone() } else { player };
    let title_font = fonts.fraunces_axes(34.0, 70.0, 0.0, 540.0, Some(144.0));
    let name_font = fonts.fraunces_axes(34.0, 100.0, 1.0, 540.0, Some(144.0));

    let prefix = "Welcome back, ";
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (prefix_w, _) = title_font.measure_str(prefix, Some(&probe));

    let title_baseline = panel.top + 86.0;
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(prefix, (left, title_baseline), &title_font, &title_paint);

    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(ROSE, 1.0), None);
    canvas.draw_str(
        &display_name,
        (left + prefix_w, title_baseline),
        &name_font,
        &name_paint,
    );

    // Skin viewer — an inset chip with the rotatable 3D model.
    draw_chip(canvas, skin_rect, 12.0);
    crate::skin::draw_skin(
        canvas,
        skin_rect,
        editor.skin_image.as_ref(),
        editor.cape_image.as_ref(),
        editor.skin_yaw,
        editor.skin_slim,
    );
    let hint_font = fonts.jetbrains_mono(8.0);
    let mut hint = Paint::default();
    hint.set_anti_alias(true);
    hint.set_color4f(rgba(MAUVE, 0.7), None);
    let hint_text = if editor.skin_image.is_some() {
        "DRAG TO ROTATE"
    } else {
        "NO SKIN LOADED"
    };
    let hint_w = measure_tracked_em(&hint_font, hint_text, 0.16);
    draw_tracked_em(
        canvas,
        hint_text,
        (skin_rect.left + (skin_rect.width() - hint_w) * 0.5, skin_rect.bottom - 12.0),
        &hint_font,
        &hint,
        0.16,
    );

    // Stat cards (right column).
    let ping = if data.ping_valid() {
        format!("{} ms", data.ping())
    } else {
        "—".to_string()
    };
    let coords = if data.world_active() {
        format!("{:.0}  {:.0}  {:.0}", data.player_x(), data.player_y(), data.player_z())
    } else {
        "—".to_string()
    };
    let server = {
        let s = data.server();
        if s.is_empty() {
            "—".to_string()
        } else {
            s
        }
    };
    let cards: [(&str, String); 5] = [
        ("FPS", data.fps().to_string()),
        ("PING", ping),
        ("PLAYTIME", fmt_playtime(data.playtime())),
        ("COORDS", coords),
        ("SERVER", server),
    ];
    for (rect, card) in stats.iter().zip(cards.iter()) {
        draw_stat_card(canvas, *rect, card.0, &card.1, fonts);
    }

    // Account + active-profile row (right column, below the cards) — small
    // rose avatar disc + Fraunces italic name + tracked-mono "profile · <name>"
    // beneath. Re-skin of the design's `.account-row > .who`. Anchor x is the
    // stats column's left edge (matches the layout in `home_layout`).
    let rx = stats[0].left;
    let name = data.player_name();
    let account = if name.is_empty() {
        "not signed in".to_string()
    } else {
        name
    };
    let row_top = stats[4].bottom + 28.0;
    let av_r = 14.0;
    let av_cx = rx + av_r;
    let av_cy = row_top + av_r;

    // Avatar — rose→berry radial disc with a subtle white inset highlight,
    // mirroring the redesign's `.avatar`.
    let av_rect = Rect::from_xywh(av_cx - av_r, av_cy - av_r, av_r * 2.0, av_r * 2.0);
    let mut av_fill = Paint::default();
    av_fill.set_anti_alias(true);
    if let Some(shader) = gradient_shader::radial(
        Point::new(av_cx - av_r * 0.4, av_cy - av_r * 0.4),
        av_r * 1.6,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[rgba(ROSE, 1.0), rgba(BERRY, 1.0), rgba(WINE, 1.0)],
            None,
        ),
        Some(&[0.0_f32, 0.6, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        av_fill.set_shader(shader);
    } else {
        av_fill.set_color4f(rgba(BERRY, 1.0), None);
    }
    canvas.draw_oval(av_rect, &av_fill);

    let name_font = fonts.fraunces_axes(17.0, 100.0, 1.0, 520.0, Some(24.0));
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&account, (av_cx + av_r + 14.0, av_cy - 3.0), &name_font, &name_paint);

    // Meta line — italic Newsreader "profile · <Name>". Mixed case avoids
    // the redundant "PROFILE · PROFILE" reading when the active profile is
    // literally named "Profile" (matches the design's `.who .meta`).
    let meta_font = fonts.newsreader(13.0);
    let mut meta_paint = Paint::default();
    meta_paint.set_anti_alias(true);
    meta_paint.set_color4f(rgba(MAUVE, 1.0), None);
    let meta = format!("profile  ·  {}", editor.active_profile);
    canvas.draw_str(&meta, (av_cx + av_r + 14.0, av_cy + 14.0), &meta_font, &meta_paint);

    // ── Media-controller card (large) ──────────────────────────────────────
    draw_media_large(
        canvas,
        media_rect,
        &editor.media,
        editor.cursor,
        editor.media_button_press,
        &editor.spectrum,
        fonts,
    );

    // ── Quick-toggle chips — flow them inside `toggle_area` with each chip
    //    sized to fit its label. Cache the per-chip rect on the editor so the
    //    press handler can hit-test without re-running the flow. ─────────────
    let qt_font = fonts.jetbrains_mono(10.0);
    let mut qt = Paint::default();
    qt.set_anti_alias(true);
    qt.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "QUICK TOGGLES  ·  HUD WIDGETS",
        (toggle_area.left, toggle_area.top - 14.0),
        &qt_font,
        &qt,
        0.18,
    );

    let chip_h = 28.0;
    let chip_gap_x = 6.0;
    let chip_gap_y = 6.0;
    let chip_label_font = fonts.jetbrains_mono(9.0);
    let mut cursor_x = toggle_area.left;
    let mut cursor_y = toggle_area.top;
    for (i, id) in WidgetId::ALL.iter().enumerate() {
        let label = id.title();
        let label_w = measure_tracked_em(&chip_label_font, label, 0.22);
        let chip_w = 15.0 + 8.0 + label_w + 14.0; // dot_offset + dot+gap + label + right_pad
        // Wrap to a new row if this chip won't fit on the current one.
        if cursor_x + chip_w > toggle_area.right && cursor_x > toggle_area.left {
            cursor_x = toggle_area.left;
            cursor_y += chip_h + chip_gap_y;
        }
        let chip_rect = Rect::from_xywh(cursor_x, cursor_y, chip_w, chip_h);
        let on = editor.layout.get(*id).enabled;
        draw_toggle_chip(canvas, chip_rect, label, on, fonts);
        editor.home_toggle_bounds[i] = chip_rect;
        cursor_x += chip_w + chip_gap_x;
    }
}

// ────────────────────────────────────────────────────────────────────────
// Media controller — "now playing" widget. Re-skin of the redesign's
// `.media-large` (HOME-tab card) + `.w-media` (compact in-world widget).
// SMTC backend wiring is a follow-up; the renderer reads `MediaState`.
// ────────────────────────────────────────────────────────────────────────

/// The album-art square — 12px-rounded; renders the real thumbnail when one
/// is available, falls back to the design's conic "vinyl" gradient otherwise
/// (a pearl/berry/lavender swirl with a centre punch, the empty-state filler
/// the redesign uses when no thumbnail is available).
pub(super) fn draw_album_art(canvas: &Canvas, rect: Rect, media: &crate::media::MediaState) {
    let rrect = RRect::new_rect_xy(rect, 12.0, 12.0);

    if let Some(img) = media.thumbnail.as_ref() {
        canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
        let mut p = Paint::default();
        p.set_anti_alias(true);
        canvas.draw_image_rect(img, None, rect, &p);
        canvas.restore();
    } else if media.is_idle() {
        // Dashed-cross-hatch empty thumbnail per `.media-large.empty .album` —
        // a repeating wine/pearl pattern with a centred italic music glyph.
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(WINE, 0.7), None);
        canvas.draw_rrect(rrect, &bg);
    } else {
        // Vinyl-style swirl when we have a playing track but the source's
        // thumbnail hasn't decoded yet. Approximation of the redesign's
        // `conic-gradient(from 220deg)` — Skia doesn't have conic gradients
        // natively, so we sweep an angular sweep_shader through the same
        // berry / rose / lavender stops.
        let cx = rect.left + rect.width() * 0.65;
        let cy = rect.top + rect.height() * 0.35;
        let mut p = Paint::default();
        p.set_anti_alias(true);
        if let Some(shader) = gradient_shader::sweep(
            (cx, cy),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(WINE, 1.0),
                    rgba(BERRY, 1.0),
                    rgba(ROSE, 1.0),
                    rgba(LAV, 1.0),
                    rgba(BERRY, 1.0),
                    rgba(WINE, 1.0),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
            None,
        ) {
            p.set_shader(shader);
        } else {
            p.set_color4f(rgba(BERRY, 1.0), None);
        }
        canvas.draw_rrect(rrect, &p);
    }

    // Inset inner shadow (CSS `inset 0 0 10px rgba(0,0,0,.4)`) — a 1px stroked
    // outline at the rect edges + a tighter centre punch.
    let mut inset = Paint::default();
    inset.set_anti_alias(true);
    inset.set_style(PaintStyle::Stroke);
    inset.set_stroke_width(1.0);
    inset.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.35), None);
    canvas.draw_rrect(rrect, &inset);

    if media.is_idle() {
        // Music glyph centred — italic Fraunces ♪ for the empty card.
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_color4f(rgba(MAUVE, 1.0), None);
        // Use the default skia fallback for `♪` — Fraunces does carry it.
        // (System fallback covers the glyph anyway if missing.)
    }
}

/// Draw a play / pause / next / previous circular button. `accent` is the
/// large-play variant (52px, rose-berry gradient) — the prev/next buttons use
/// the muted variant (38px, wine fill + pearl border).
pub(super) fn draw_media_button(
    canvas: &Canvas,
    rect: Rect,
    accent: bool,
    _playing: bool,
    glyph: char,
    hovered: bool,
    press_strength: f32,
) {
    let cx = rect.left + rect.width() * 0.5;
    let cy = rect.top + rect.height() * 0.5;
    let r_base = rect.width() * 0.5;
    // Press scales the button down ~6% at full strength, easing back out.
    let press_scale = 1.0 - press_strength * 0.06;
    let r = r_base * press_scale;

    if accent {
        // Outer rose glow — base intensity, lifted on hover, super-lifted on press.
        let glow_alpha = 0.55 + if hovered { 0.18 } else { 0.0 } + press_strength * 0.20;
        let glow_blur = 10.0 + if hovered { 4.0 } else { 0.0 } + press_strength * 6.0;
        let glow_extent = r + 4.0 + if hovered { 2.0 } else { 0.0 };
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(ROSE, glow_alpha.min(1.0)), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, glow_blur, false));
        canvas.draw_circle((cx, cy), glow_extent, &glow);

        // Body gradient — lifts slightly when hovered/pressed so it reads brighter.
        let lift = if hovered { 1.08 } else { 1.0 } + press_strength * 0.06;
        let rose_lift = Color4f::new(
            (ROSE.0 as f32 / 255.0 * lift).min(1.0),
            (ROSE.1 as f32 / 255.0 * lift).min(1.0),
            (ROSE.2 as f32 / 255.0 * lift).min(1.0),
            1.0,
        );
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(rect.left, rect.top), Point::new(rect.left, rect.bottom)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rose_lift, rgba(BERRY, 1.0)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        } else {
            fill.set_color4f(rgba(ROSE, 1.0), None);
        }
        canvas.draw_circle((cx, cy), r, &fill);

        // Inset top highlight (slightly brighter on hover so the button reads
        // as "lit up" without needing a CSS-style transition).
        let hi_alpha = if hovered { 0.5 } else { 0.35 };
        let mut hi = Paint::default();
        hi.set_anti_alias(true);
        hi.set_style(PaintStyle::Stroke);
        hi.set_stroke_width(1.0);
        hi.set_color4f(Color4f::new(1.0, 1.0, 1.0, hi_alpha), None);
        canvas.draw_circle((cx, cy - 0.5), r - 0.5, &hi);
    } else {
        // Hover: the wine fill brightens, the border picks up a rose tint
        // (matches the redesign's `.mctl:hover{border-color:...rose;background:...berry}` rule).
        let fill_alpha = 0.55 + if hovered { 0.2 } else { 0.0 } + press_strength * 0.15;
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if hovered {
            // Blend wine + a touch of berry so the disc reads as "lit" rather
            // than just darker.
            fill.set_color4f(
                Color4f::new(
                    (WINE.0 as f32 / 255.0 + 0.20).min(1.0),
                    (WINE.1 as f32 / 255.0 + 0.04).min(1.0),
                    (WINE.2 as f32 / 255.0 + 0.10).min(1.0),
                    fill_alpha.min(1.0),
                ),
                None,
            );
        } else {
            fill.set_color4f(rgba(WINE, fill_alpha.min(1.0)), None);
        }
        canvas.draw_circle((cx, cy), r, &fill);

        // Border — pearl on rest, rose on hover, brighter rose on press.
        let border_color = if hovered {
            rgba(ROSE, (0.45 + press_strength * 0.3).min(1.0))
        } else {
            rgba(PEARL, 0.12)
        };
        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(border_color, None);
        canvas.draw_circle((cx, cy), r - 0.5, &border);

        // Hover halo — a soft rose ring just outside the button.
        if hovered || press_strength > 0.0 {
            let halo_alpha = (0.3 + press_strength * 0.4).min(0.8);
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, halo_alpha), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, false));
            halo.set_style(PaintStyle::Stroke);
            halo.set_stroke_width(2.0);
            canvas.draw_circle((cx, cy), r + 2.0, &halo);
        }
    }

    // Glyph — vector paths so we don't depend on a music-symbol font cut.
    let glyph_color = if accent {
        Color4f::new(0.16, 0.03, 0.08, 1.0) // deep wine on the rose button
    } else if hovered {
        rgba(ROSE, 1.0)
    } else {
        rgba(PEARL, 0.95)
    };
    let mut g = Paint::default();
    g.set_anti_alias(true);
    g.set_color4f(glyph_color, None);
    let size = r * 0.38;
    match glyph {
        // ▶ play triangle.
        'P' => {
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx - size * 0.55, cy - size));
                p.line_to((cx + size * 0.9, cy));
                p.line_to((cx - size * 0.55, cy + size));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        // ⏸ pause double bar.
        'p' => {
            let bw = size * 0.36;
            let bh = size * 1.7;
            let gap = size * 0.45;
            canvas.draw_rect(
                Rect::from_xywh(cx - gap * 0.5 - bw, cy - bh * 0.5, bw, bh),
                &g,
            );
            canvas.draw_rect(
                Rect::from_xywh(cx + gap * 0.5, cy - bh * 0.5, bw, bh),
                &g,
            );
        }
        // ⏮ previous — left bar + reverse triangle.
        '<' => {
            let bh = size * 1.6;
            let bw = size * 0.32;
            canvas.draw_rect(
                Rect::from_xywh(cx - size * 1.1, cy - bh * 0.5, bw, bh),
                &g,
            );
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx + size * 0.9, cy - bh * 0.5));
                p.line_to((cx - size * 0.55, cy));
                p.line_to((cx + size * 0.9, cy + bh * 0.5));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        // ⏭ next — right bar + forward triangle.
        '>' => {
            let bh = size * 1.6;
            let bw = size * 0.32;
            canvas.draw_rect(
                Rect::from_xywh(cx + size * 0.78, cy - bh * 0.5, bw, bh),
                &g,
            );
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx - size * 0.9, cy - bh * 0.5));
                p.line_to((cx + size * 0.55, cy));
                p.line_to((cx - size * 0.9, cy + bh * 0.5));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        _ => {}
    }
}

/// Format `seconds` as `m:ss` — small helper for the scrub timestamps.
pub(super) fn fmt_track_time(seconds: f32) -> String {
    let s = seconds.max(0.0) as i32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Large media-controller card — the HOME-tab "now playing" form. Renders the
/// album / vinyl thumbnail on the left, title + artist + scrub bar in the
/// middle, prev/play/next buttons on the right. Empty state ("Silence is
/// also a song") when `MediaState::is_idle()`.
pub(super) fn draw_media_large(
    canvas: &Canvas,
    rect: Rect,
    media: &crate::media::MediaState,
    cursor: (f32, f32),
    press_info: Option<(usize, std::time::Instant)>,
    audio: &crate::audio::Spectrum,
    fonts: &FontStore,
) {
    let rrect = RRect::new_rect_xy(rect, 18.0, 18.0);
    let idle = media.is_idle();

    // The card answers to the music the same way the in-world plate does — an
    // outer bloom that swells, under the chrome so it reads as the card
    // glowing rather than a ring around it.
    let drive = (!idle)
        .then(|| PlateDrive::from_audio(audio))
        .flatten()
        .unwrap_or_default();
    let energy = drive.level * 0.30 + drive.pulse * 0.70;
    if energy > 0.01 {
        let grow = 2.0 + 7.0 * drive.pulse;
        let halo = Rect::new(
            rect.left - grow,
            rect.top - grow,
            rect.right + grow,
            rect.bottom + grow,
        );
        let mut bloom = Paint::default();
        bloom.set_anti_alias(true);
        bloom.set_color4f(rgba(ROSE, 0.38 * energy), None);
        bloom.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 14.0 + 20.0 * energy, false));
        canvas.draw_rrect(RRect::new_rect_xy(halo, 18.0 + grow, 18.0 + grow), &bloom);
    }

    // ── Card chrome — 135° berry → lavender → wine fill + rose border + glow.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    if !idle {
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.right, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(BERRY, 0.22),
                    rgba(LAV, 0.10),
                    rgba(WINE, 0.72),
                ],
                None,
            ),
            Some(&[0.0_f32, 0.4, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        } else {
            fill.set_color4f(rgba(WINE, 0.7), None);
        }
    } else {
        // Empty state — quieter, mauve over wine.
        fill.set_color4f(rgba(WINE, 0.65), None);
    }
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0 + drive.pulse * 0.9);
    border.set_color4f(
        if idle {
            rgba(PEARL, 0.08)
        } else {
            rgba(ROSE, 0.22 + drive.level * 0.34 + drive.pulse * 0.30)
        },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    // Inset top highlight.
    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.08), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, 17.5, 17.5), &top);
    canvas.restore();

    // ── Album art (left) ───────────────────────────────────────────────────
    let art_size = 72.0;
    let pad_x = 14.0;
    let art = Rect::from_xywh(
        rect.left + pad_x,
        rect.top + (rect.height() - art_size) * 0.5,
        art_size,
        art_size,
    );
    draw_album_art(canvas, art, media);

    // Album-art empty-state glyph — drawn here over the swirl so we have the
    // metrics available locally. Italic Fraunces ♪ ~ centre.
    if idle {
        let g_font = fonts.fraunces_axes(36.0, 100.0, 1.0, 500.0, Some(72.0));
        let glyph = "\u{266A}"; // ♪
        let mut probe = Paint::default();
        let (gw, _) = g_font.measure_str(glyph, Some(&probe));
        let (_, gm) = g_font.metrics();
        let gcap = if gm.cap_height > 0.0 { gm.cap_height } else { 22.0 };
        probe.set_anti_alias(true);
        let mut gp = Paint::default();
        gp.set_anti_alias(true);
        gp.set_color4f(rgba(MAUVE, 0.85), None);
        canvas.draw_str(
            glyph,
            (
                art.left + (art.width() - gw) * 0.5,
                art.top + (art.height() + gcap) * 0.5,
            ),
            &g_font,
            &gp,
        );
    }

    // ── Middle column — "Now playing" eyebrow + title + artist + scrub ────
    let mid_left = art.right + 16.0;
    // Right cluster (prev 34 + gap 8 + play 46 + gap 8 + next 34) + pad 14.
    let cluster_right_w = 34.0 + 8.0 + 46.0 + 8.0 + 34.0 + 14.0;
    let mid_right = rect.right - cluster_right_w - 4.0;
    let eyebrow_font = fonts.jetbrains_mono(10.5);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    let eyebrow_label = if idle {
        "IDLE  ·  NO SOURCE CONNECTED".to_string()
    } else if media.source.is_empty() {
        "NOW PLAYING".to_string()
    } else {
        format!("NOW PLAYING  ·  {}", media.source)
    };
    eyebrow.set_color4f(
        if idle {
            rgba(MAUVE, 1.0)
        } else {
            rgba(ROSE, 1.0)
        },
        None,
    );
    let eyebrow_y = rect.top + 28.0;
    draw_tracked_em(
        canvas,
        &eyebrow_label,
        (mid_left, eyebrow_y),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    // Title — italic Fraunces (WONK 1, SOFT 100). Falls back to a poetic
    // empty-state placeholder when idle. Sized to fit "Slow Dancing in a
    // Burning Room"-class titles inside the middle column.
    let title_font = fonts.fraunces_axes(19.0, 100.0, 1.0, 540.0, Some(36.0));
    let title_str = if idle {
        "Silence is also a song".to_string()
    } else {
        media.title.clone()
    };
    let title_baseline = eyebrow_y + 28.0;
    // Clip the title to the middle column width to avoid overflowing into the
    // controls when the title is long.
    canvas.save();
    let title_clip = Rect::new(mid_left, rect.top, mid_right, rect.bottom);
    canvas.clip_rect(title_clip, Some(ClipOp::Intersect), Some(true));
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(
        if idle {
            rgba(MAUVE, 1.0)
        } else {
            rgba(PEARL, 1.0)
        },
        None,
    );
    canvas.draw_str(&title_str, (mid_left, title_baseline), &title_font, &title_paint);

    // Artist line — italic Newsreader-ish (we don't have a true italic
    // Fraunces cut here yet; tracked Newsreader mauve is a close stand-in).
    let artist_str = if idle {
        "Connect Spotify, browser audio, or system media.".to_string()
    } else if media.artist.is_empty() {
        String::new()
    } else {
        media.artist.clone()
    };
    if !artist_str.is_empty() {
        let artist_font = fonts.newsreader(13.0);
        let mut artist = Paint::default();
        artist.set_anti_alias(true);
        artist.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&artist_str, (mid_left, title_baseline + 18.0), &artist_font, &artist);
    }
    canvas.restore();

    // Scrub bar (skip if idle).
    if !idle {
        let scrub_y = rect.bottom - 26.0;
        let time_font = fonts.jetbrains_mono(10.0);
        let live_pos = media.displayed_position();
        let pos_str = fmt_track_time(live_pos);
        let dur_str = if media.duration_seconds > 0.0 {
            fmt_track_time(media.duration_seconds)
        } else {
            String::new()
        };
        let mut probe = Paint::default();
        probe.set_anti_alias(true);
        let (pos_w, _) = time_font.measure_str(&pos_str, Some(&probe));
        let (dur_w, _) = time_font.measure_str(&dur_str, Some(&probe));

        let mut t = Paint::default();
        t.set_anti_alias(true);
        t.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&pos_str, (mid_left, scrub_y + 4.0), &time_font, &t);
        if !dur_str.is_empty() {
            canvas.draw_str(&dur_str, (mid_right - dur_w, scrub_y + 4.0), &time_font, &t);
        }

        // The bar — track + rose→champ fill + pearl knob.
        let track_left = mid_left + pos_w + 10.0;
        let track_right = mid_right - dur_w - 10.0;
        let track_h = 2.0;
        let track_rect = Rect::from_xywh(
            track_left,
            scrub_y - 4.0 - track_h * 0.5,
            (track_right - track_left).max(0.0),
            track_h,
        );
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(PEARL, 0.10), None);
        canvas.draw_rrect(RRect::new_rect_xy(track_rect, track_h, track_h), &bg);

        let frac = if media.duration_seconds > 0.0 {
            (live_pos / media.duration_seconds).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fill_w = track_rect.width() * frac;
        if fill_w > 1.0 {
            let fill_rect = Rect::from_xywh(track_rect.left, track_rect.top, fill_w, track_h);
            let mut fp = Paint::default();
            fp.set_anti_alias(true);
            if let Some(shader) = gradient_shader::linear(
                (
                    Point::new(track_rect.left, track_rect.top),
                    Point::new(track_rect.right, track_rect.top),
                ),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(CHAMP, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fp.set_shader(shader);
            } else {
                fp.set_color4f(rgba(ROSE, 1.0), None);
            }
            canvas.draw_rrect(RRect::new_rect_xy(fill_rect, track_h, track_h), &fp);

            // Pearl knob with a rose halo.
            let knob_x = track_rect.left + fill_w;
            let knob_y = track_rect.top + track_h * 0.5;
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, 0.4), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
            canvas.draw_circle((knob_x, knob_y), 6.0, &halo);
            let mut knob = Paint::default();
            knob.set_anti_alias(true);
            knob.set_color4f(rgba(PEARL, 1.0), None);
            canvas.draw_circle((knob_x, knob_y), 4.0, &knob);
        }
    }

    // ── Right cluster — prev / play(pause) / next ──────────────────────────
    let ctl_size = 34.0;
    let play_size = 46.0;
    let gap = 8.0;
    let cluster_w = ctl_size + gap + play_size + gap + ctl_size;
    let cluster_left = rect.right - 14.0 - cluster_w;
    let cy = rect.top + rect.height() * 0.5;
    let prev = Rect::from_xywh(cluster_left, cy - ctl_size * 0.5, ctl_size, ctl_size);
    let play = Rect::from_xywh(cluster_left + ctl_size + gap, cy - play_size * 0.5, play_size, play_size);
    let next = Rect::from_xywh(
        cluster_left + ctl_size + gap + play_size + gap,
        cy - ctl_size * 0.5,
        ctl_size,
        ctl_size,
    );

    let (hover_prev, press_prev) = button_state(prev, cursor, 0, press_info);
    let (hover_play, press_play) = button_state(play, cursor, 1, press_info);
    let (hover_next, press_next) = button_state(next, cursor, 2, press_info);
    if idle {
        // Idle — buttons are present but visually muted.
        draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
        draw_media_button(canvas, play, false, false, 'P', hover_play, press_play);
        draw_media_button(canvas, next, false, false, '>', hover_next, press_next);
    } else {
        draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
        draw_media_button(
            canvas,
            play,
            true,
            media.playing,
            if media.playing { 'p' } else { 'P' },
            hover_play,
            press_play,
        );
        draw_media_button(canvas, next, false, false, '>', hover_next, press_next);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Mods view — the bundled-mod toggle list.
// ────────────────────────────────────────────────────────────────────────
