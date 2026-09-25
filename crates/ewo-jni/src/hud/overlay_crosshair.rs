use super::*;

/// Hit-test rectangles for the CROSSHAIR editor — both `draw_crosshair_view`
/// and the press / drag handlers consume this.
pub(super) struct CrosshairLayout {
    /// Outer card (drawn with the glass-panel chrome).
    pub(super) card: Rect,
    /// The three small 1:1 preview panes (idle / target / reach), centred
    /// in a single horizontal strip below the title.
    pub(super) preview_idle: Rect,
    pub(super) preview_target: Rect,
    pub(super) preview_reach: Rect,
    /// Big enable toggle below the preview strip.
    pub(super) enable_toggle: Rect,
    /// Three small boolean toggles in a row below the enable pill.
    pub(super) bool_dot: Rect,
    pub(super) bool_outline: Rect,
    pub(super) bool_aa: Rect,
    /// Shape sliders, top-to-bottom on the LEFT column.
    pub(super) arm_length: Rect,
    pub(super) arm_gap: Rect,
    pub(super) arm_thickness: Rect,
    pub(super) dot_size: Rect,
    pub(super) outline_thickness: Rect,
    pub(super) reach_distance: Rect,
    /// Four colour-state swatches above the picker on the RIGHT column.
    pub(super) swatch_idle: Rect,
    pub(super) swatch_target: Rect,
    pub(super) swatch_reach: Rect,
    pub(super) swatch_outline: Rect,
    /// HSV picker controls.
    pub(super) sv_square: Rect,
    pub(super) hue_strip: Rect,
    pub(super) alpha_strip: Rect,
    /// Hex readout below the picker (read-only — for now).
    pub(super) hex_display: Rect,
}

pub(super) fn crosshair_layout(w: f32, h: f32) -> CrosshairLayout {
    let card_w = (w - 240.0).min(1500.0).max(900.0);
    let card_h = (h - 140.0).min(880.0).max(620.0);
    let card_x = (w - card_w) * 0.5;
    let card_y = 100.0;
    let card = Rect::from_xywh(card_x, card_y, card_w, card_h);

    let body_top = card.top + 74.0; // below title + subtitle
    let body_left = card.left + 36.0;
    let body_right = card.right - 36.0;
    let body_bottom = card.bottom - 28.0;
    let body_w = body_right - body_left;

    let mut y = body_top;

    // ── Preview row — 3 panes side-by-side, centred at 1:1 scale. ─────
    let preview_pane_w = 220.0;
    let preview_pane_h = 110.0;
    let preview_gap = 14.0;
    let preview_row_w = 3.0 * preview_pane_w + 2.0 * preview_gap;
    let preview_left = body_left + (body_w - preview_row_w) * 0.5;
    let preview_idle = Rect::from_xywh(preview_left, y, preview_pane_w, preview_pane_h);
    let preview_target = Rect::from_xywh(
        preview_left + preview_pane_w + preview_gap,
        y, preview_pane_w, preview_pane_h,
    );
    let preview_reach = Rect::from_xywh(
        preview_left + 2.0 * (preview_pane_w + preview_gap),
        y, preview_pane_w, preview_pane_h,
    );
    // The labels sit *under* each pane; budget +16 for them.
    y += preview_pane_h + 22.0;

    // ── Enable pill, full body width. ─────────────────────────────────
    let enable_toggle = Rect::from_xywh(body_left, y, body_w, 42.0);
    y = enable_toggle.bottom + 18.0;

    // ── Bottom: two columns. Left = shape; right = colour. ───────────
    let col_gap = 40.0;
    let col_w = (body_w - col_gap) * 0.5;
    let col_left = body_left;
    let col_right_start = body_left + col_w + col_gap;
    let col_right_end = body_right;

    // ── LEFT column: shape ────────────────────────────────────────────
    let row_h = 28.0;
    let slider_h = 22.0;
    let slider_gap = 16.0;
    let third_w = (col_w - 16.0) / 3.0;
    let mut ly = y;
    let bool_dot = Rect::new(col_left, ly, col_left + third_w, ly + row_h);
    let bool_outline = Rect::new(col_left + third_w + 8.0, ly, col_left + 2.0 * third_w + 8.0, ly + row_h);
    let bool_aa = Rect::new(col_left + 2.0 * third_w + 16.0, ly, col_left + col_w, ly + row_h);
    ly = bool_dot.bottom + slider_gap + 4.0;

    let mk_slider_left = |ly: f32| Rect::new(col_left, ly + 18.0, col_left + col_w, ly + 18.0 + slider_h);
    let arm_length = mk_slider_left(ly);
    ly = arm_length.bottom + slider_gap;
    let arm_gap_r = mk_slider_left(ly);
    ly = arm_gap_r.bottom + slider_gap;
    let arm_thickness = mk_slider_left(ly);
    ly = arm_thickness.bottom + slider_gap;
    let dot_size = mk_slider_left(ly);
    ly = dot_size.bottom + slider_gap;
    let outline_thickness = mk_slider_left(ly);
    ly = outline_thickness.bottom + slider_gap;
    let reach_distance = mk_slider_left(ly);
    let _ = ly;

    // ── RIGHT column: colour ──────────────────────────────────────────
    let mut ry = y;
    // Four swatches in a row.
    let sw_h = 32.0;
    let sw_w = (col_right_end - col_right_start - 24.0) / 4.0;
    let swatch_idle = Rect::from_xywh(col_right_start, ry, sw_w, sw_h);
    let swatch_target = Rect::from_xywh(col_right_start + sw_w + 8.0, ry, sw_w, sw_h);
    let swatch_reach = Rect::from_xywh(col_right_start + 2.0 * (sw_w + 8.0), ry, sw_w, sw_h);
    let swatch_outline = Rect::from_xywh(col_right_start + 3.0 * (sw_w + 8.0), ry, sw_w, sw_h);
    ry = swatch_idle.bottom + 18.0;

    // HSV picker block — SV square + vertical Hue + vertical Alpha + hex.
    // Total height capped to what's left so it stays inside the body.
    let picker_max_h = (body_bottom - ry - 36.0).max(180.0); // 36 reserved for hex line
    let picker_h = picker_max_h.min(240.0);
    let strip_w = 22.0;
    let strip_gap = 12.0;
    // SV square is square — width == height.
    let sv_side = picker_h;
    let sv_square = Rect::from_xywh(col_right_start, ry, sv_side, sv_side);
    let hue_strip = Rect::from_xywh(sv_square.right + strip_gap, ry, strip_w, sv_side);
    let alpha_strip = Rect::from_xywh(hue_strip.right + strip_gap, ry, strip_w, sv_side);
    ry = sv_square.bottom + 14.0;

    // Hex line — sits centred under the picker, full column width.
    let hex_display = Rect::from_xywh(col_right_start, ry, col_right_end - col_right_start, 22.0);

    CrosshairLayout {
        card,
        preview_idle,
        preview_target,
        preview_reach,
        enable_toggle,
        bool_dot,
        bool_outline,
        bool_aa,
        arm_length,
        arm_gap: arm_gap_r,
        arm_thickness,
        dot_size,
        outline_thickness,
        reach_distance,
        swatch_idle,
        swatch_target,
        swatch_reach,
        swatch_outline,
        sv_square,
        hue_strip,
        alpha_strip,
        hex_display,
    }
}

/// Inclusive numeric range for a 1D shape-slider — the cursor's x position
/// in the track maps linearly across this span. (HSV picker controls don't
/// share this — they have their own 2D / 1D handling in `drag_crosshair_slider`.)
pub(super) fn crosshair_slider_range(slot: CrosshairSlider) -> (f32, f32) {
    use CrosshairSlider::*;
    match slot {
        ArmLength => (0.0, 16.0),
        ArmGap => (0.0, 10.0),
        ArmThickness => (1.0, 6.0),
        DotSize => (1.0, 6.0),
        OutlineThickness => (0.0, 2.0),
        ReachDistance => (1.5, 5.0),
        SvSquare | HueStrip | AlphaStrip => (0.0, 1.0), // unused — handled directly
    }
}

pub(super) fn set_shape_slider(cfg: &mut crate::crosshair::CrosshairConfig, slot: CrosshairSlider, value: f32) {
    use CrosshairSlider::*;
    match slot {
        ArmLength => cfg.arm_length = value,
        ArmGap => cfg.arm_gap = value,
        ArmThickness => cfg.arm_thickness = value,
        DotSize => cfg.dot_size = value,
        OutlineThickness => cfg.outline_thickness = value,
        ReachDistance => cfg.reach_distance = value,
        // Picker controls don't go through this path.
        SvSquare | HueStrip | AlphaStrip => {}
    }
}

// ── HSV ↔ RGB ───────────────────────────────────────────────────────────

/// Hue (0..360), Saturation (0..1), Value (0..1) → 8-bit RGB.
pub(super) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// 8-bit RGB → (H, S, V). Returns H=0 for grey colours (chroma 0 — H is
/// ambiguous there).
pub(super) fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, s, v)
}

/// Pull the selected swatch's RGBA into the picker's HSV state. Called
/// when the user clicks a swatch — gives the picker its starting point so
/// the SV square / hue strip / alpha strip are positioned at the slot's
/// current colour.
pub(super) fn sync_picker_from_slot(ui: &mut CrosshairUi, cfg: &crate::crosshair::CrosshairConfig) {
    let rgba = color_for_slot(cfg, ui.selected_color);
    // Special-case greys (delta=0): keep the user's hue so dragging the
    // value down to black and back up to grey doesn't reset their hue.
    let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
    if s > 1e-3 {
        ui.picker_h = h;
    }
    ui.picker_s = s;
    ui.picker_v = v;
    ui.picker_a = rgba[3] as f32 / 255.0;
}

/// Bake the picker's HSV state back into the selected swatch's RGBA. Run
/// after every drag step so the in-world crosshair updates live.
pub(super) fn write_picker_to_slot(ui: &CrosshairUi, cfg: &mut crate::crosshair::CrosshairConfig) {
    let rgb = hsv_to_rgb(ui.picker_h, ui.picker_s, ui.picker_v);
    let slot = color_for_slot_mut(cfg, ui.selected_color);
    slot[0] = rgb[0];
    slot[1] = rgb[1];
    slot[2] = rgb[2];
    slot[3] = (ui.picker_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

pub(super) fn color_for_slot(cfg: &crate::crosshair::CrosshairConfig, slot: CrosshairColorSlot) -> [u8; 4] {
    match slot {
        CrosshairColorSlot::Idle => cfg.color_idle,
        CrosshairColorSlot::Target => cfg.color_target,
        CrosshairColorSlot::Reach => cfg.color_reach,
        CrosshairColorSlot::Outline => cfg.outline_color,
    }
}

pub(super) fn color_for_slot_mut(cfg: &mut crate::crosshair::CrosshairConfig, slot: CrosshairColorSlot) -> &mut [u8; 4] {
    match slot {
        CrosshairColorSlot::Idle => &mut cfg.color_idle,
        CrosshairColorSlot::Target => &mut cfg.color_target,
        CrosshairColorSlot::Reach => &mut cfg.color_reach,
        CrosshairColorSlot::Outline => &mut cfg.outline_color,
    }
}

impl Editor {
    /// Press inside the CROSSHAIR view: flip a toggle, select a colour
    /// slot, or start a slider / picker drag. Toggle flips persist
    /// immediately; slider + picker drags persist on release.
    pub(super) fn crosshair_press(&mut self, x: f32, y: f32) {
        let layout = crosshair_layout(self.window.0, self.window.1);

        // Big "Enable Custom Crosshair" pill.
        if point_in(layout.enable_toggle, x, y) {
            self.crosshair.enabled = !self.crosshair.enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_dot, x, y) {
            self.crosshair.dot_enabled = !self.crosshair.dot_enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_outline, x, y) {
            self.crosshair.outline_enabled = !self.crosshair.outline_enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_aa, x, y) {
            self.crosshair.anti_alias = !self.crosshair.anti_alias;
            crate::crosshair::save(&self.crosshair);
            return;
        }

        // Colour swatches — selecting one re-syncs the HSV picker to it.
        for (rect, slot) in [
            (layout.swatch_idle, CrosshairColorSlot::Idle),
            (layout.swatch_target, CrosshairColorSlot::Target),
            (layout.swatch_reach, CrosshairColorSlot::Reach),
            (layout.swatch_outline, CrosshairColorSlot::Outline),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.selected_color = slot;
                sync_picker_from_slot(&mut self.crosshair_ui, &self.crosshair);
                return;
            }
        }

        // Shape sliders — start a 1D drag.
        for (rect, slot) in [
            (layout.arm_length, CrosshairSlider::ArmLength),
            (layout.arm_gap, CrosshairSlider::ArmGap),
            (layout.arm_thickness, CrosshairSlider::ArmThickness),
            (layout.dot_size, CrosshairSlider::DotSize),
            (layout.outline_thickness, CrosshairSlider::OutlineThickness),
            (layout.reach_distance, CrosshairSlider::ReachDistance),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.drag = Some(slot);
                self.drag_crosshair_slider(slot, x, y);
                return;
            }
        }

        // HSV picker — SV square (2D), Hue strip (1D), Alpha strip (1D).
        for (rect, slot) in [
            (layout.sv_square, CrosshairSlider::SvSquare),
            (layout.hue_strip, CrosshairSlider::HueStrip),
            (layout.alpha_strip, CrosshairSlider::AlphaStrip),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.drag = Some(slot);
                self.drag_crosshair_slider(slot, x, y);
                return;
            }
        }
    }

    /// Update the value bound to whatever control is currently dragged.
    /// `x` + `y` come straight from the cursor; the 2D SV square uses
    /// both, the 1D strips/sliders use one each.
    pub(super) fn drag_crosshair_slider(&mut self, slot: CrosshairSlider, x: f32, y: f32) {
        let layout = crosshair_layout(self.window.0, self.window.1);
        match slot {
            // 1D shape sliders.
            CrosshairSlider::ArmLength
            | CrosshairSlider::ArmGap
            | CrosshairSlider::ArmThickness
            | CrosshairSlider::DotSize
            | CrosshairSlider::OutlineThickness
            | CrosshairSlider::ReachDistance => {
                let track = match slot {
                    CrosshairSlider::ArmLength => layout.arm_length,
                    CrosshairSlider::ArmGap => layout.arm_gap,
                    CrosshairSlider::ArmThickness => layout.arm_thickness,
                    CrosshairSlider::DotSize => layout.dot_size,
                    CrosshairSlider::OutlineThickness => layout.outline_thickness,
                    CrosshairSlider::ReachDistance => layout.reach_distance,
                    _ => unreachable!(),
                };
                let inner_left = track.left + 10.0;
                let inner_right = track.right - 10.0;
                let span = (inner_right - inner_left).max(1.0);
                let frac = ((x - inner_left) / span).clamp(0.0, 1.0);
                let (min, max) = crosshair_slider_range(slot);
                let value = min + frac * (max - min);
                set_shape_slider(&mut self.crosshair, slot, value);
            }
            CrosshairSlider::SvSquare => {
                let sq = layout.sv_square;
                let fx = ((x - sq.left) / sq.width().max(1.0)).clamp(0.0, 1.0);
                let fy = ((y - sq.top) / sq.height().max(1.0)).clamp(0.0, 1.0);
                // x = saturation, y = (1 - value) so the bright corner sits
                // at the top-left of the square (canonical SV picker layout).
                self.crosshair_ui.picker_s = fx;
                self.crosshair_ui.picker_v = 1.0 - fy;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
            CrosshairSlider::HueStrip => {
                let strip = layout.hue_strip;
                let fy = ((y - strip.top) / strip.height().max(1.0)).clamp(0.0, 1.0);
                self.crosshair_ui.picker_h = fy * 360.0;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
            CrosshairSlider::AlphaStrip => {
                let strip = layout.alpha_strip;
                let fy = ((y - strip.top) / strip.height().max(1.0)).clamp(0.0, 1.0);
                // Top = opaque, bottom = transparent — matches Photoshop /
                // most colour pickers.
                self.crosshair_ui.picker_a = 1.0 - fy;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
        }
    }
}

pub(super) fn draw_crosshair_view(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let layout = crosshair_layout(w, h);

    // ── Outer card chrome ───────────────────────────────────────────────
    draw_iw_shell(canvas, layout.card, 18.0);

    // Title row.
    let head_font = fonts.fraunces_axes(30.0, 100.0, 1.0, 500.0, Some(40.0));
    let mut head_paint = Paint::default();
    head_paint.set_anti_alias(true);
    head_paint.set_color4f(rgba(PEARL, 0.98), None);
    canvas.draw_str(
        "Crosshair",
        (layout.card.left + 36.0, layout.card.top + 44.0),
        &head_font,
        &head_paint,
    );

    let sub_font = fonts.fraunces_axes(15.0, 30.0, 0.0, 380.0, Some(20.0));
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color4f(rgba(MAUVE, 0.85), None);
    let (hw, _) = head_font.measure_str("Crosshair", Some(&head_paint));
    canvas.draw_str(
        "always rendered, colour adapts to reach",
        (layout.card.left + 36.0 + hw + 14.0, layout.card.top + 42.0),
        &sub_font,
        &sub_paint,
    );

    // Right-side hairline pip with the selected profile name, mirroring
    // the other dashboard tabs.
    let pip_font = fonts.jetbrains_mono(11.0);
    let mut pip_paint = Paint::default();
    pip_paint.set_anti_alias(true);
    pip_paint.set_color4f(rgba(MAUVE, 0.55), None);
    let pip = format!("PROFILE  ·  {}", editor.active_profile.to_uppercase());
    let pw = measure_tracked_em(&pip_font, &pip, 0.18);
    draw_tracked_em(
        canvas,
        &pip,
        (layout.card.right - 36.0 - pw, layout.card.top + 42.0),
        &pip_font,
        &pip_paint,
        0.18,
    );

    // ── Preview row ─────────────────────────────────────────────────────
    draw_crosshair_preview(canvas, &layout, &editor.crosshair, fonts);

    // ── Enable toggle (full-width pill) ─────────────────────────────────
    draw_crosshair_enable_pill(canvas, layout.enable_toggle, editor.crosshair.enabled, fonts);

    // ── Section labels (left = SHAPE, right = COLOUR) ──────────────────
    let section_font = fonts.jetbrains_mono(10.0);
    let mut section_paint = Paint::default();
    section_paint.set_anti_alias(true);
    section_paint.set_color4f(rgba(MAUVE, 0.6), None);
    draw_tracked_em(
        canvas, "SHAPE",
        (layout.bool_dot.left, layout.bool_dot.top - 10.0),
        &section_font, &section_paint, 0.22,
    );
    draw_tracked_em(
        canvas, "COLOUR",
        (layout.swatch_idle.left, layout.swatch_idle.top - 10.0),
        &section_font, &section_paint, 0.22,
    );

    // ── Three boolean toggles (LEFT col) ────────────────────────────────
    draw_crosshair_bool(canvas, layout.bool_dot, "Center dot", editor.crosshair.dot_enabled, fonts);
    draw_crosshair_bool(canvas, layout.bool_outline, "Outline", editor.crosshair.outline_enabled, fonts);
    draw_crosshair_bool(canvas, layout.bool_aa, "Anti-alias", editor.crosshair.anti_alias, fonts);

    // ── Shape sliders (LEFT col) ────────────────────────────────────────
    draw_crosshair_slider(
        canvas, layout.arm_length, "Arm length", editor.crosshair.arm_length,
        crosshair_slider_range(CrosshairSlider::ArmLength), "px", 0, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.arm_gap, "Center gap", editor.crosshair.arm_gap,
        crosshair_slider_range(CrosshairSlider::ArmGap), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.arm_thickness, "Arm thickness", editor.crosshair.arm_thickness,
        crosshair_slider_range(CrosshairSlider::ArmThickness), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.dot_size, "Center dot size", editor.crosshair.dot_size,
        crosshair_slider_range(CrosshairSlider::DotSize), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.outline_thickness, "Outline thickness", editor.crosshair.outline_thickness,
        crosshair_slider_range(CrosshairSlider::OutlineThickness), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.reach_distance, "Reach distance", editor.crosshair.reach_distance,
        crosshair_slider_range(CrosshairSlider::ReachDistance), "blocks", 1, fonts,
    );

    // ── Colour swatches (RIGHT col) ─────────────────────────────────────
    draw_crosshair_swatch(canvas, layout.swatch_idle, "Idle", editor.crosshair.color_idle, editor.crosshair_ui.selected_color == CrosshairColorSlot::Idle, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_target, "Target", editor.crosshair.color_target, editor.crosshair_ui.selected_color == CrosshairColorSlot::Target, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_reach, "In reach", editor.crosshair.color_reach, editor.crosshair_ui.selected_color == CrosshairColorSlot::Reach, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_outline, "Outline", editor.crosshair.outline_color, editor.crosshair_ui.selected_color == CrosshairColorSlot::Outline, fonts);

    // ── HSV picker — SV square + Hue strip + Alpha strip ───────────────
    let ui = &editor.crosshair_ui;
    let current_rgba = color_for_slot(&editor.crosshair, ui.selected_color);
    let pure_hue_rgb = hsv_to_rgb(ui.picker_h, 1.0, 1.0);
    draw_sv_square(canvas, layout.sv_square, pure_hue_rgb, ui.picker_s, ui.picker_v);
    draw_hue_strip(canvas, layout.hue_strip, ui.picker_h);
    draw_alpha_strip(canvas, layout.alpha_strip, [current_rgba[0], current_rgba[1], current_rgba[2]], ui.picker_a);

    // ── Hex readout below the picker ────────────────────────────────────
    let hex_font = fonts.jetbrains_mono(13.0);
    let mut hex_paint = Paint::default();
    hex_paint.set_anti_alias(true);
    hex_paint.set_color4f(rgba(PEARL, 0.9), None);
    let hex_str = format!(
        "#{:02X}{:02X}{:02X}  ·  α {:>3}",
        current_rgba[0], current_rgba[1], current_rgba[2], current_rgba[3]
    );
    let (hw2, _) = hex_font.measure_str(&hex_str, Some(&hex_paint));
    canvas.draw_str(
        &hex_str,
        (
            layout.hex_display.left + (layout.hex_display.width() - hw2) * 0.5,
            layout.hex_display.top + 16.0,
        ),
        &hex_font,
        &hex_paint,
    );
}

/// Three side-by-side preview panes (Idle / Target / In-Reach), each
/// showing the crosshair at **true 1:1 scale** over the actual live
/// game framebuffer. The composite step in `lib.rs` cuts these rects
/// out of the overlay frost (`Editor::live_game_cutouts`) and re-blits
/// the un-frosted snapshot underneath, so the pane interior is the
/// user's real game — we just clear it here in the Skia overlay surface
/// and paint the crosshair on top.
pub(super) fn draw_crosshair_preview(
    canvas: &Canvas,
    layout: &CrosshairLayout,
    cfg: &crate::crosshair::CrosshairConfig,
    fonts: &FontStore,
) {
    use skia_safe::{BlendMode, RRect};

    let panes = [
        (layout.preview_idle, crate::crosshair::CrosshairState::Idle, "IDLE"),
        (layout.preview_target, crate::crosshair::CrosshairState::Target, "TARGET"),
        (layout.preview_reach, crate::crosshair::CrosshairState::Reach, "IN REACH"),
    ];

    let label_font = fonts.jetbrains_mono(10.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 0.75), None);

    for (pane, state, label) in panes {
        let rrect = RRect::new_rect_xy(pane, 10.0, 10.0);
        let saved = canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

        // The Skia offscreen surface was tinted black-22% earlier in
        // `hud::draw` (the overlay "focus mode" dim). That dim covers
        // the whole screen including these panes — which would dull
        // the live game we're trying to show. Punch it back out with
        // BlendMode::Src + fully-transparent black so the pane interior
        // becomes alpha=0 again.
        let mut clear = Paint::default();
        clear.set_blend_mode(BlendMode::Src);
        clear.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.0), None);
        canvas.draw_rect(pane, &clear);

        // Crosshair at exact pane centre, 1:1 scale — *true to life*.
        // Drawn into the Skia overlay surface; once composited, it
        // sits over the live game pixels the composite step blitted
        // into this same rect.
        let cx = (pane.left + pane.right) * 0.5;
        let cy = (pane.top + pane.bottom) * 0.5;
        crate::crosshair::draw(canvas, cx.round(), cy.round(), cfg, state);

        canvas.restore_to_count(saved);

        // Rim — 1px pearl-tinted border, sits on top of everything.
        let mut rim = Paint::default();
        rim.set_anti_alias(true);
        rim.set_style(PaintStyle::Stroke);
        rim.set_stroke_width(1.0);
        rim.set_color4f(rgba(MAUVE, 0.45), None);
        canvas.draw_rrect(rrect, &rim);

        // Label centred under the pane.
        let lw = measure_tracked_em(&label_font, label, 0.22);
        draw_tracked_em(
            canvas, label,
            (pane.left + (pane.width() - lw) * 0.5, pane.bottom + 14.0),
            &label_font, &label_paint, 0.22,
        );
    }
}

/// Big "Enable Custom Crosshair" pill. On = rose fill + pearl label,
/// off = dim track + mauve label.
pub(super) fn draw_crosshair_enable_pill(canvas: &Canvas, rect: Rect, on: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, rect.height() * 0.5, rect.height() * 0.5);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(ROSE, 0.32) } else { rgba(MAUVE, 0.12) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(ROSE, 0.7) } else { rgba(MAUVE, 0.3) }, None);
    canvas.draw_rrect(rrect, &border);

    let font = fonts.fraunces(17.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 0.98) } else { rgba(MAUVE, 0.85) }, None);
    let label = if on {
        "Enabled — vanilla crosshair suppressed"
    } else {
        "Disabled — vanilla crosshair shown"
    };
    let (tw, _) = font.measure_str(label, Some(&text));
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - tw) * 0.5, rect.top + rect.height() * 0.5 + 6.0),
        &font,
        &text,
    );
}

/// Inline boolean toggle pill — label + a small on/off mark.
pub(super) fn draw_crosshair_bool(canvas: &Canvas, rect: Rect, label: &str, on: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, 8.0, 8.0);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(LAV, 0.18) } else { rgba(MAUVE, 0.08) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(LAV, 0.55) } else { rgba(MAUVE, 0.22) }, None);
    canvas.draw_rrect(rrect, &border);

    let dot_r = 5.0;
    let dot_cx = rect.left + 14.0;
    let dot_cy = rect.top + rect.height() * 0.5;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(if on { rgba(ROSE, 0.95) } else { rgba(MAUVE, 0.4) }, None);
    canvas.draw_circle((dot_cx, dot_cy), dot_r, &dot);

    let font = fonts.newsreader(14.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 0.95) } else { rgba(MAUVE, 0.78) }, None);
    canvas.draw_str(label, (dot_cx + 12.0, dot_cy + 5.0), &font, &text);
}

/// Slider track with label above + value (and optional unit) on the right.
pub(super) fn draw_crosshair_slider(
    canvas: &Canvas,
    rect: Rect,
    label: &str,
    value: f32,
    range: (f32, f32),
    unit: &str,
    decimals: usize,
    fonts: &FontStore,
) {
    let label_font = fonts.newsreader(13.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 0.92), None);
    canvas.draw_str(label, (rect.left, rect.top - 6.0), &label_font, &label_paint);

    // Right-aligned value tag.
    let val_str = if unit.is_empty() {
        format!("{:.*}", decimals, value)
    } else {
        format!("{:.*} {}", decimals, value, unit)
    };
    let val_font = fonts.jetbrains_mono(12.0);
    let mut val_paint = Paint::default();
    val_paint.set_anti_alias(true);
    val_paint.set_color4f(rgba(CHAMP, 0.95), None);
    let (vw, _) = val_font.measure_str(&val_str, Some(&val_paint));
    canvas.draw_str(&val_str, (rect.right - vw, rect.top - 6.0), &val_font, &val_paint);

    // Track + filled portion + knob.
    let (min, max) = range;
    let frac = ((value - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
    let inner_left = rect.left + 10.0;
    let inner_right = rect.right - 10.0;
    let track_y = rect.top + rect.height() * 0.5;
    let track = Rect::new(inner_left, track_y - 1.5, inner_right, track_y + 1.5);

    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(MAUVE, 0.22), None);
    canvas.draw_rrect(skia_safe::RRect::new_rect_xy(track, 1.5, 1.5), &track_paint);

    let fill_x = inner_left + (inner_right - inner_left) * frac;
    let fill = Rect::new(inner_left, track_y - 1.5, fill_x, track_y + 1.5);
    let mut fill_paint = Paint::default();
    fill_paint.set_anti_alias(true);
    fill_paint.set_color4f(rgba(ROSE, 0.85), None);
    canvas.draw_rrect(skia_safe::RRect::new_rect_xy(fill, 1.5, 1.5), &fill_paint);

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 0.95), None);
    canvas.draw_circle((fill_x, track_y), 6.0, &knob);
}

/// Colour-state swatch — fills the pill with the slot's current colour and
/// outlines it (thicker outline = currently selected).
pub(super) fn draw_crosshair_swatch(canvas: &Canvas, rect: Rect, label: &str, color: [u8; 4], selected: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, 8.0, 8.0);
    // A small chess-pattern background ghosts through transparent colours so
    // the user knows the alpha channel is in play.
    let mut ghost = Paint::default();
    ghost.set_anti_alias(true);
    ghost.set_color4f(rgba(MAUVE, 0.12), None);
    canvas.draw_rrect(rrect, &ghost);

    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(crate::crosshair::color4f(color), None);
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(if selected { 2.0 } else { 1.0 });
    border.set_color4f(if selected { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.45) }, None);
    canvas.draw_rrect(rrect, &border);

    let font = fonts.jetbrains_mono(10.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    // Pick legible text colour: light on dark fills, dark on light fills.
    let luma = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
    text.set_color4f(
        if luma > 140.0 { rgba((15, 0, 8), 0.95) } else { rgba(PEARL, 0.95) },
        None,
    );
    let (tw, _) = font.measure_str(label, Some(&text));
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - tw) * 0.5, rect.top + rect.height() * 0.5 + 4.0),
        &font,
        &text,
    );
}

// ── HSV picker controls ──────────────────────────────────────────────────

/// Saturation/Value square — paints the canonical 3-pass colour-picker
/// square (solid pure hue → horizontal white→transparent → vertical
/// transparent→black) plus a ring cursor at the current (S, V).
pub(super) fn draw_sv_square(canvas: &Canvas, rect: Rect, pure_hue_rgb: [u8; 3], s: f32, v: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    // 1. Solid pure hue.
    let mut hue_fill = Paint::default();
    hue_fill.set_color4f(
        Color4f::new(
            pure_hue_rgb[0] as f32 / 255.0,
            pure_hue_rgb[1] as f32 / 255.0,
            pure_hue_rgb[2] as f32 / 255.0,
            1.0,
        ),
        None,
    );
    canvas.draw_rect(rect, &hue_fill);

    // 2. Horizontal white→transparent (saturation axis).
    let mut sat_paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[Color4f::new(1.0, 1.0, 1.0, 1.0), Color4f::new(1.0, 1.0, 1.0, 0.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        sat_paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &sat_paint);

    // 3. Vertical transparent→black (value axis).
    let mut val_paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[Color4f::new(0.0, 0.0, 0.0, 0.0), Color4f::new(0.0, 0.0, 0.0, 1.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        val_paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &val_paint);

    canvas.restore_to_count(saved);

    // Outline + cursor ring.
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    let cx = rect.left + s * rect.width();
    let cy = rect.top + (1.0 - v) * rect.height();
    let mut ring = Paint::default();
    ring.set_anti_alias(true);
    ring.set_style(PaintStyle::Stroke);
    ring.set_stroke_width(2.0);
    // White outline, then a thin black inner ring — readable on both light
    // and dark backgrounds inside the square.
    ring.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_circle((cx, cy), 7.0, &ring);
    ring.set_stroke_width(1.0);
    ring.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_circle((cx, cy), 5.5, &ring);
}

/// Hue strip — vertical 6-stop rainbow gradient (red→yellow→green→cyan→
/// blue→magenta→red). Cursor knob at y proportional to H/360.
pub(super) fn draw_hue_strip(canvas: &Canvas, rect: Rect, h: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    let stops = [
        Color4f::new(1.0, 0.0, 0.0, 1.0), // 0°
        Color4f::new(1.0, 1.0, 0.0, 1.0), // 60°
        Color4f::new(0.0, 1.0, 0.0, 1.0), // 120°
        Color4f::new(0.0, 1.0, 1.0, 1.0), // 180°
        Color4f::new(0.0, 0.0, 1.0, 1.0), // 240°
        Color4f::new(1.0, 0.0, 1.0, 1.0), // 300°
        Color4f::new(1.0, 0.0, 0.0, 1.0), // 360°
    ];
    let mut paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(&stops, None),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &paint);

    canvas.restore_to_count(saved);

    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    // Cursor — a horizontal bar straddling the strip at the picked hue.
    let cy = rect.top + (h / 360.0) * rect.height();
    let bar = Rect::new(rect.left - 3.0, cy - 2.5, rect.right + 3.0, cy + 2.5);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
    knob.set_style(PaintStyle::Stroke);
    knob.set_stroke_width(1.0);
    knob.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
}

/// Alpha strip — checkerboard backdrop + vertical gradient from the
/// current colour (opaque, top) to the same colour (transparent, bottom).
/// Cursor at y = (1 - alpha) * height.
pub(super) fn draw_alpha_strip(canvas: &Canvas, rect: Rect, rgb: [u8; 3], a: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    // Checkerboard — tiny squares so the user sees alpha as "see-through".
    let tile = 6.0;
    let cols = (rect.width() / tile).ceil() as i32;
    let rows = (rect.height() / tile).ceil() as i32;
    let mut light = Paint::default();
    light.set_color4f(Color4f::new(0.78, 0.78, 0.78, 1.0), None);
    let mut dark = Paint::default();
    dark.set_color4f(Color4f::new(0.55, 0.55, 0.55, 1.0), None);
    canvas.draw_rect(rect, &light);
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) & 1 == 0 {
                continue;
            }
            let tx = rect.left + col as f32 * tile;
            let ty = rect.top + row as f32 * tile;
            canvas.draw_rect(
                Rect::from_xywh(tx, ty, tile, tile),
                &dark,
            );
        }
    }

    // Solid colour → transparent overlay.
    let opaque = Color4f::new(
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    );
    let transparent = Color4f::new(opaque.r, opaque.g, opaque.b, 0.0);
    let mut paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(&[opaque, transparent], None),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &paint);

    canvas.restore_to_count(saved);

    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    let cy = rect.top + (1.0 - a) * rect.height();
    let bar = Rect::new(rect.left - 3.0, cy - 2.5, rect.right + 3.0, cy + 2.5);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
    knob.set_style(PaintStyle::Stroke);
    knob.set_stroke_width(1.0);
    knob.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
}
