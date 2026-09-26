use super::*;

/// Hit-rects for the PVP tab — three sections of rows.
pub(super) struct PvpLayout {
    pub(super) panel: Rect,
    /// General-section toggles, in the order
    /// `[jump, jump-bar, hit-range, totem-count, floating-health]`.
    pub(super) general_toggles: [Rect; 5],
    /// Per-tier rows — [tier index]{ sound chip, volume slider }.
    pub(super) tier_sound: [Rect; 5],
    pub(super) tier_volume: [Rect; 5],
    /// Per-zone rows — [zone index]{ enable toggle, min/max sliders, sound, vol }.
    pub(super) zone_enable: [Rect; 3],
    pub(super) zone_min: [Rect; 3],
    pub(super) zone_max: [Rect; 3],
    pub(super) zone_sound: [Rect; 3],
    pub(super) zone_volume: [Rect; 3],
}

/// Lay out the PVP-tab panel + every control. Deterministic in `(w, h)`, so
/// the renderer and the press-handler agree.
pub(super) fn pvp_layout(w: f32, h: f32) -> PvpLayout {
    const PANEL_W: f32 = 740.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 78.0;
    const SECTION_GAP: f32 = 18.0;
    const SECTION_HEAD_H: f32 = 26.0;
    const ROW_H: f32 = 32.0;

    let general_rows = 5;
    let tier_rows = 5;
    let zone_rows = 3;
    let body_h = SECTION_HEAD_H + ROW_H * general_rows as f32
        + SECTION_GAP + SECTION_HEAD_H + ROW_H * tier_rows as f32
        + SECTION_GAP + SECTION_HEAD_H + ROW_H * zone_rows as f32;
    let panel_h = PAD * 2.0 + HEADER_H + body_h;
    let px = (w - PANEL_W) * 0.5;
    let py = ((h - panel_h) * 0.5).max(70.0); // never above the tab strip
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let content_x = px + PAD;
    let content_w = PANEL_W - PAD * 2.0;
    let toggle_w = 38.0;
    let toggle_h = 20.0;

    // General toggles — right-edge.
    let mut general_toggles = [empty_rect(); 5];
    let general_top = py + PAD + HEADER_H + SECTION_HEAD_H;
    for i in 0..5 {
        let row_top = general_top + i as f32 * ROW_H;
        general_toggles[i] = Rect::from_xywh(
            content_x + content_w - toggle_w,
            row_top + (ROW_H - toggle_h) * 0.5,
            toggle_w,
            toggle_h,
        );
    }

    // Per-tier rows — left: label text (handled by draw), middle: sound chip,
    // right: volume slider.
    let mut tier_sound = [empty_rect(); 5];
    let mut tier_volume = [empty_rect(); 5];
    let tier_top = general_top + ROW_H * 5.0 + SECTION_GAP + SECTION_HEAD_H;
    let label_w = 150.0;
    let sound_chip_w = 130.0;
    let gap = 16.0;
    let vol_left = content_x + label_w + gap + sound_chip_w + gap;
    for i in 0..5 {
        let row_top = tier_top + i as f32 * ROW_H;
        tier_sound[i] = Rect::from_xywh(
            content_x + label_w + gap,
            row_top + 4.0,
            sound_chip_w,
            ROW_H - 8.0,
        );
        tier_volume[i] = Rect::from_xywh(
            vol_left,
            row_top + (ROW_H - 20.0) * 0.5,
            content_x + content_w - vol_left,
            20.0,
        );
    }

    // Per-zone rows — { label | enable | min | max | sound chip | vol slider }.
    let mut zone_enable = [empty_rect(); 3];
    let mut zone_min = [empty_rect(); 3];
    let mut zone_max = [empty_rect(); 3];
    let mut zone_sound = [empty_rect(); 3];
    let mut zone_volume = [empty_rect(); 3];
    let zone_top = tier_top + ROW_H * 5.0 + SECTION_GAP + SECTION_HEAD_H;
    let z_label_w = 60.0;
    let z_toggle_w = 30.0;
    let z_toggle_h = 16.0;
    let z_slider_w = 100.0;
    let z_sound_w = 100.0;
    let z_gap = 10.0;
    for i in 0..3 {
        let row_top = zone_top + i as f32 * ROW_H;
        let mut x = content_x + z_label_w;
        zone_enable[i] = Rect::from_xywh(
            x,
            row_top + (ROW_H - z_toggle_h) * 0.5,
            z_toggle_w,
            z_toggle_h,
        );
        x += z_toggle_w + z_gap;
        zone_min[i] = Rect::from_xywh(x, row_top + (ROW_H - 20.0) * 0.5, z_slider_w, 20.0);
        x += z_slider_w + z_gap;
        zone_max[i] = Rect::from_xywh(x, row_top + (ROW_H - 20.0) * 0.5, z_slider_w, 20.0);
        x += z_slider_w + z_gap;
        zone_sound[i] = Rect::from_xywh(x, row_top + 4.0, z_sound_w, ROW_H - 8.0);
        x += z_sound_w + z_gap;
        zone_volume[i] = Rect::from_xywh(
            x,
            row_top + (ROW_H - 20.0) * 0.5,
            content_x + content_w - x,
            20.0,
        );
    }

    PvpLayout {
        panel,
        general_toggles,
        tier_sound,
        tier_volume,
        zone_enable,
        zone_min,
        zone_max,
        zone_sound,
        zone_volume,
    }
}

/// Draw a sound-cycle chip — a Velvet pill with the sound's name, clickable
/// to cycle to the next sound. Simpler than a full portal dropdown and
/// composes cleanly in this dense layout.
pub(super) fn draw_pvp_sound_chip(canvas: &Canvas, rect: Rect, sound: crate::pvp::PvpSound, fonts: &FontStore) {
    let rr = RRect::new_rect_xy(rect, rect.height() * 0.4, rect.height() * 0.4);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(rr, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.22), None);
    canvas.draw_rrect(rr, &border);

    let font = fonts.jetbrains_mono(11.0);
    let label = sound.label();
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color4f(rgba(PEARL, 0.92), None);
    let (lw, _) = font.measure_str(label, Some(&p));
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 8.0 };
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - lw) * 0.5 - 4.0, rect.top + rect.height() * 0.5 + cap * 0.5),
        &font,
        &p,
    );
    // A tiny "▾" hint at the right edge.
    let mut hint = Paint::default();
    hint.set_anti_alias(true);
    hint.set_color4f(rgba(MAUVE, 0.85), None);
    canvas.draw_str(
        "▾",
        (rect.right - 12.0, rect.top + rect.height() * 0.5 + cap * 0.5),
        &font,
        &hint,
    );
}

/// Draw a small min/max-distance slider — sized for the dense zone row. Knob
/// position is `frac` (0..1); current value displayed to the right.
pub(super) fn draw_pvp_distance_slider(canvas: &Canvas, area: Rect, value: f32, fonts: &FontStore) {
    const RANGE_MIN: f32 = 0.0;
    const RANGE_MAX: f32 = 3.5;
    let cy = area.top + area.height() * 0.5;
    let value_w = 36.0;
    let track_left = area.left + 4.0;
    let track_right = area.right - value_w;
    let track_h = 3.0;

    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h, track_h), &tp);

    let span = (RANGE_MAX - RANGE_MIN).max(0.001);
    let frac = ((value - RANGE_MIN) / span).clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(ROSE, 0.95), None);
    canvas.draw_circle((knob_x, cy), 5.0, &knob);

    let font = fonts.jetbrains_mono(10.0);
    let val = format!("{:.1}", value);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, 0.92), None);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.0 };
    canvas.draw_str(&val, (track_right + 6.0, cy + cap * 0.5), &font, &vp);
}

/// Draw a small volume slider — fixed 0..1 range, knob in pearl.
pub(super) fn draw_pvp_volume_slider(canvas: &Canvas, area: Rect, value: f32, fonts: &FontStore) {
    let cy = area.top + area.height() * 0.5;
    let value_w = 40.0;
    let track_left = area.left + 4.0;
    let track_right = area.right - value_w;
    let track_h = 3.0;

    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h, track_h), &tp);

    let frac = value.clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);
    if knob_x > track_left + 1.0 {
        let fill = Rect::from_xywh(track_left, cy - track_h * 0.5, knob_x - track_left, track_h);
        let mut fp = Paint::default();
        fp.set_anti_alias(true);
        fp.set_color4f(rgba(LAV, 0.8), None);
        canvas.draw_rrect(RRect::new_rect_xy(fill, track_h, track_h), &fp);
    }

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_circle((knob_x, cy), 5.0, &knob);

    let font = fonts.jetbrains_mono(10.0);
    let val = format!("{:.2}", value);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, 0.92), None);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.0 };
    canvas.draw_str(&val, (track_right + 6.0, cy + cap * 0.5), &font, &vp);
}

/// Draw the PVP view — Velvet panel + three sections of editing controls.
pub(super) fn draw_pvp(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let layout = pvp_layout(w, h);
    let cfg = &editor.pvp;
    draw_chip(canvas, layout.panel, 16.0);

    let left = layout.panel.left + 24.0;

    // Header — eyebrow + title + subhead.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "PVP UTILS",
        (left, layout.panel.top + 36.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let title_font = fonts.fraunces_axes(26.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(
        "Combat indicators & sound cues",
        (left, layout.panel.top + 68.0),
        &title_font,
        &title,
    );

    let body_left = layout.panel.left + 24.0;
    let body_right = layout.panel.right - 24.0;
    let row_h: f32 = 32.0;

    // ── Section: GENERAL ─────────────────────────────────────────────────
    let general_section_top = layout.general_toggles[0].top - (row_h - 20.0) * 0.5 - 26.0;
    draw_pvp_section_label(canvas, "GENERAL", body_left, general_section_top + 18.0, fonts);

    let general_labels = [
        "Jump reset",
        "Jump reset bar",
        "Hit range",
        "Totem pop counter",
        "Floating health",
    ];
    let general_states = [
        cfg.jump_reset_enabled,
        cfg.jump_reset_bar_enabled,
        cfg.hit_range_enabled,
        cfg.totem_count_enabled,
        cfg.floating_health_enabled,
    ];
    let label_font = fonts.newsreader(14.0);
    for i in 0..5 {
        let cy = layout.general_toggles[i].top + layout.general_toggles[i].height() * 0.5;
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(general_labels[i], (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_panel_toggle(canvas, layout.general_toggles[i], general_states[i]);
    }

    // ── Section: SOUNDS PER TIER ─────────────────────────────────────────
    let tiers_section_top = layout.tier_sound[0].top - 26.0;
    draw_pvp_section_label(canvas, "SOUNDS PER TIER", body_left, tiers_section_top + 18.0, fonts);

    for (i, tier) in crate::pvp::Tier::ALL.iter().enumerate() {
        let slot = cfg.sound_for_tier(*tier);
        let cy = layout.tier_sound[i].top + layout.tier_sound[i].height() * 0.5;
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(tier.label(), (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_pvp_sound_chip(canvas, layout.tier_sound[i], slot.sound, fonts);
        draw_pvp_volume_slider(canvas, layout.tier_volume[i], slot.volume, fonts);
    }

    // ── Section: HIT-RANGE ZONES ─────────────────────────────────────────
    let zones_section_top = layout.zone_enable[0].top - 26.0;
    draw_pvp_section_label(canvas, "HIT-RANGE ZONES", body_left, zones_section_top + 18.0, fonts);

    for i in 0..3 {
        let z = cfg.zone(i);
        let cy = layout.zone_enable[i].top + layout.zone_enable[i].height() * 0.5;
        let zlabel = format!("Zone {}", i + 1);
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(&zlabel, (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_panel_toggle(canvas, layout.zone_enable[i], z.enabled);
        draw_pvp_distance_slider(canvas, layout.zone_min[i], z.min_dist, fonts);
        draw_pvp_distance_slider(canvas, layout.zone_max[i], z.max_dist, fonts);
        draw_pvp_sound_chip(canvas, layout.zone_sound[i], z.sound, fonts);
        draw_pvp_volume_slider(canvas, layout.zone_volume[i], z.volume, fonts);
    }

    // Quiet the unused body_right warning when we add more controls later.
    let _ = body_right;
}

pub(super) fn draw_pvp_section_label(canvas: &Canvas, label: &str, x: f32, y: f32, fonts: &FontStore) {
    let font = fonts.jetbrains_mono(10.0);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(canvas, label, (x, y), &font, &p, 0.20);
}

// ────────────────────────────────────────────────────────────────────────
// Modules view — the EwoClient module toggle list (Phase G).
// ────────────────────────────────────────────────────────────────────────
