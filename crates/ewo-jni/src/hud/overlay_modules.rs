use super::*;

/// Per-module hit-rects on the MODULES view. Sorted by category for grouping;
/// `catalog_index` carries the canonical REGISTRY position so the slider-
/// drag + toggle dispatch can still look up the module without walking the
/// REGISTRY in sync.
pub(super) struct ModuleRow {
    /// The module's index in [`catalog::REGISTRY`]. Used by toggle + slider
    /// dispatch to identify which module the row belongs to.
    pub(super) catalog_index: usize,
    /// The whole row — base content plus any setting sliders below it.
    pub(super) row: Rect,
    /// The on/off toggle pill.
    pub(super) toggle: Rect,
    /// Slider hit-areas, one per module setting (FOV Control has one).
    pub(super) sliders: Vec<Rect>,
}

/// A category header rendered between module groups.
pub(super) struct ModuleSection {
    pub(super) label: &'static str,
    /// Baseline-y for the header label.
    pub(super) label_y: f32,
}

/// Velvet accent for a module category — the row's colour dot.
pub(super) fn module_category_color(category: catalog::ModuleCategory) -> (u8, u8, u8) {
    match category {
        catalog::ModuleCategory::Visual => LAV,
        catalog::ModuleCategory::Camera => ROSE,
        catalog::ModuleCategory::Movement => CHAMP,
    }
}

/// Category iteration order — VISUAL first, then CAMERA, then MOVEMENT.
/// Stable so the rendered tab order doesn't shift as new modules ship.
pub(super) const MODULE_CATEGORY_ORDER: [catalog::ModuleCategory; 3] = [
    catalog::ModuleCategory::Visual,
    catalog::ModuleCategory::Camera,
    catalog::ModuleCategory::Movement,
];

/// The Modules-view panel, the section headers between groups, and one
/// [`ModuleRow`] per catalog module (in grouped display order). Deterministic
/// in the window size, so renderer + hit-tester agree.
/// 2-column compact module grid + scrollable panel. The panel is always sized
/// to the available viewport (less the tab strip); content height is reported
/// separately so the renderer + scroll clamp know the overflow.
///
/// Layout: a panel with a fixed header (eyebrow + title + counts), then a
/// scrollable body. The body is organised into category groups
/// (VISUAL → CAMERA → MOVEMENT); within each group, modules pair up into a
/// 2-column row band. The two columns are independent — a left module with
/// settings sliders can be taller than the right one; the band height is
/// `max(left, right)`. Sliders sit directly below their row in the same column.
pub(super) fn modules_layout(w: f32, h: f32) -> (Rect, Vec<ModuleSection>, Vec<ModuleRow>, f32) {
    const PANEL_W: f32 = 920.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 76.0;
    const ROW_H: f32 = 60.0; // every row is the same compact height now.
    const TOGGLE_W: f32 = 38.0;
    const TOGGLE_H: f32 = 20.0;
    const COL_GAP: f32 = 16.0;
    const ROW_GAP: f32 = 10.0;
    const SECTION_TOP_GAP: f32 = 22.0;
    const SECTION_LABEL_GAP: f32 = 14.0;

    let col_w = (PANEL_W - PAD * 2.0 - COL_GAP) / 2.0;

    // Group modules by category, in MODULE_CATEGORY_ORDER, preserving each
    // module's REGISTRY index for the dispatch path.
    let mut groups: Vec<Vec<usize>> = Vec::with_capacity(MODULE_CATEGORY_ORDER.len());
    for cat in MODULE_CATEGORY_ORDER {
        let mut group: Vec<usize> = catalog::REGISTRY
            .iter()
            .enumerate()
            .filter_map(|(i, m)| (m.category == cat).then_some(i))
            .collect();
        group.sort();
        groups.push(group);
    }

    // Panel size — fill the viewport with comfortable margins. Content scrolls
    // when it exceeds the available body height.
    let px = (w - PANEL_W) * 0.5;
    let py = 64.0; // sit just under the tab strip
    let panel_h = (h - py - 24.0).max(400.0);
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let mut rows: Vec<ModuleRow> = Vec::with_capacity(catalog::REGISTRY.len());
    let mut sections: Vec<ModuleSection> = Vec::with_capacity(MODULE_CATEGORY_ORDER.len());
    let body_top = py + PAD + HEADER_H;
    let mut ry = body_top;

    for (group_i, group) in groups.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let cat = MODULE_CATEGORY_ORDER[group_i];
        ry += SECTION_TOP_GAP;
        sections.push(ModuleSection {
            label: cat.label(),
            label_y: ry,
        });
        ry += SECTION_LABEL_GAP;

        // Pair the group's modules into 2-column row bands. Every row is the
        // same uniform `ROW_H` tall — sliders live in the right-click popover
        // now, not inline, so band height is constant.
        let left_col_x = px + PAD;
        let right_col_x = px + PAD + col_w + COL_GAP;
        for chunk in group.chunks(2) {
            let left_idx = chunk[0];
            let right_idx = chunk.get(1).copied();

            let row = Rect::from_xywh(left_col_x, ry, col_w, ROW_H);
            let toggle = Rect::from_xywh(
                left_col_x + col_w - 14.0 - TOGGLE_W,
                ry + (ROW_H - TOGGLE_H) * 0.5,
                TOGGLE_W,
                TOGGLE_H,
            );
            rows.push(ModuleRow { catalog_index: left_idx, row, toggle, sliders: Vec::new() });

            if let Some(idx) = right_idx {
                let row = Rect::from_xywh(right_col_x, ry, col_w, ROW_H);
                let toggle = Rect::from_xywh(
                    right_col_x + col_w - 14.0 - TOGGLE_W,
                    ry + (ROW_H - TOGGLE_H) * 0.5,
                    TOGGLE_W,
                    TOGGLE_H,
                );
                rows.push(ModuleRow { catalog_index: idx, row, toggle, sliders: Vec::new() });
            }

            ry += ROW_H + ROW_GAP;
        }
    }

    // Content height that needs to scroll — from body_top down to the last row.
    let content_h = (ry - body_top).max(0.0);
    (panel, sections, rows, content_h)
}

/// Draw the Modules view — a Velvet feature list, one row per EwoClient module:
/// a category dot, the name + description, an on/off toggle, and a slider for
/// any setting the module carries.
pub(super) fn draw_modules(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let (panel, sections, rows, content_h) = modules_layout(w, h);
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 24.0;

    // ── Fixed header (eyebrow + counts + title) — drawn over the scrolled
    //    body, so the user always knows what tab they're on. ───────────────
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "MODULES",
        (left, panel.top + 36.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let on = (0..catalog::REGISTRY.len())
        .filter(|&i| editor.modules.get(i).enabled)
        .count();
    let count_str = format!("{} / {} ON", on, catalog::REGISTRY.len());
    let count_w = measure_tracked_em(&eyebrow_font, &count_str, 0.22);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &count_str,
        (panel.right - 24.0 - count_w, panel.top + 36.0),
        &eyebrow_font,
        &count_paint,
        0.22,
    );

    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("EwoClient modules", (left, panel.top + 70.0), &title_font, &title);

    // ── Scrollable body — clip to the panel inner rect, translate by the
    //    editor's scroll offset, then render sections + rows in logical (un-
    //    scrolled) coordinates. The press handler offsets clicks by the same
    //    amount so hit-tests against ModuleRow rects line up. ───────────────
    let body_top = panel.top + 24.0 + 76.0;
    let body_clip = Rect::new(panel.left + 4.0, body_top - 8.0, panel.right - 4.0, panel.bottom - 4.0);
    let max_scroll = (content_h - (panel.bottom - body_top - 8.0)).max(0.0);
    let scroll_y = editor.modules_scroll.clamp(0.0, max_scroll);

    canvas.save();
    canvas.clip_rect(body_clip, Some(ClipOp::Intersect), Some(true));
    canvas.translate((0.0, -scroll_y));

    let section_font = fonts.jetbrains_mono(11.0);
    let mut section_paint = Paint::default();
    section_paint.set_anti_alias(true);
    section_paint.set_color4f(rgba(ROSE, 0.85), None);
    for section in &sections {
        draw_tracked_em(
            canvas,
            section.label,
            (left, section.label_y),
            &section_font,
            &section_paint,
            0.22,
        );
    }

    // Row cards — compact, no description: LED + cat eyebrow above name,
    // optional keybind chip + on/off toggle on the right. Wine card fill is
    // opaque enough that in-world widgets behind it (HudEditor mode aside)
    // don't bleed through.
    let cat_font = fonts.jetbrains_mono(9.5);
    let name_font = fonts.fraunces_axes(18.0, 60.0, 0.0, 540.0, Some(36.0));
    let chip_font = fonts.jetbrains_mono(10.0);
    for row in rows.iter() {
        let idx = row.catalog_index;
        let def = &catalog::REGISTRY[idx];
        let st = editor.modules.get(idx);

        // Card chrome — the row IS the card now (2-col layout uses col_w),
        // height covers ROW_H + any setting sliders below it.
        let card = row.row;
        let card_rr = RRect::new_rect_xy(card, 12.0, 12.0);

        // Opaque wine base on *every* card, enabled or not.
        //
        // The enabled card used to be berry at alpha 0.18 with no base, while
        // the disabled one was wine at 0.72. That made an enabled card's
        // appearance a function of whatever happened to be behind it: over
        // bright sky it washed out to near-white, over dirt it read rich. Two
        // cards in the same state looked like different states depending on
        // where the player was standing, which is the "washed out" complaint
        // in its purest form. The accent now rides *on top* of an opaque base,
        // so "enabled" looks the same everywhere.
        let mut card_fill = Paint::default();
        card_fill.set_anti_alias(true);
        card_fill.set_color4f(rgba(WINE, 0.72), None);
        canvas.draw_rrect(card_rr, &card_fill);
        if st.enabled {
            let mut accent = Paint::default();
            accent.set_anti_alias(true);
            accent.set_color4f(
                Color4f::new(
                    BERRY.0 as f32 / 255.0,
                    BERRY.1 as f32 / 255.0,
                    BERRY.2 as f32 / 255.0,
                    0.22,
                ),
                None,
            );
            canvas.draw_rrect(card_rr, &accent);
        }
        let mut card_border = Paint::default();
        card_border.set_anti_alias(true);
        card_border.set_style(PaintStyle::Stroke);
        card_border.set_stroke_width(1.0);
        card_border.set_color4f(
            if st.enabled {
                rgba(ROSE, 0.28)
            } else {
                rgba(PEARL, 0.08)
            },
            None,
        );
        canvas.draw_rrect(card_rr, &card_border);

        // Category LED.
        let led_cx = card.left + 18.0;
        let led_cy = card.top + 30.0;
        let led_color = module_category_color(def.category);
        if st.enabled {
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(led_color, 0.55), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
            canvas.draw_circle((led_cx, led_cy), 7.0, &halo);
        }
        let mut led = Paint::default();
        led.set_anti_alias(true);
        led.set_color4f(
            if st.enabled {
                rgba(led_color, 1.0)
            } else {
                Color4f::new(0x3A as f32 / 255.0, 0x1A as f32 / 255.0, 0x23 as f32 / 255.0, 1.0)
            },
            None,
        );
        canvas.draw_circle((led_cx, led_cy), 4.0, &led);

        // Category eyebrow above the name.
        let text_left = card.left + 36.0;
        let mut cat_paint = Paint::default();
        cat_paint.set_anti_alias(true);
        cat_paint.set_color4f(rgba(MAUVE, 0.95), None);
        draw_tracked_em(
            canvas,
            def.category.label(),
            (text_left, card.top + 20.0),
            &cat_font,
            &cat_paint,
            0.24,
        );

        // Name — clipped to leave room for the chip + toggle on the right.
        let right_reserve = 14.0 + row.toggle.width()
            + editor
                .keybind_label_for(def.id)
                .map(|l| {
                    9.0 * 2.0 + measure_tracked_em(&chip_font, &l, 0.18) + 10.0
                })
                .unwrap_or(0.0);
        let name_clip = Rect::new(
            text_left,
            card.top + 24.0,
            card.right - right_reserve - 6.0,
            card.top + 48.0,
        );
        canvas.save();
        canvas.clip_rect(name_clip, Some(ClipOp::Intersect), Some(true));
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color4f(
            if st.enabled {
                rgba(PEARL, 1.0)
            } else {
                rgba(MAUVE, 0.75)
            },
            None,
        );
        canvas.draw_str(def.name, (text_left, card.top + 44.0), &name_font, &name_paint);
        canvas.restore();

        // Optional keybind chip left of the toggle.
        if let Some(label) = editor.keybind_label_for(def.id) {
            let chip_pad_x = 9.0;
            let chip_h = 22.0;
            let label_w = measure_tracked_em(&chip_font, &label, 0.18);
            let chip_w = chip_pad_x * 2.0 + label_w;
            let chip_x = row.toggle.left - 10.0 - chip_w;
            let chip_y = row.toggle.top + (row.toggle.height() - chip_h) * 0.5;
            let chip_rect = Rect::from_xywh(chip_x, chip_y, chip_w, chip_h);
            let chip_rr = RRect::new_rect_xy(chip_rect, 6.0, 6.0);

            let mut chip_fill = Paint::default();
            chip_fill.set_anti_alias(true);
            chip_fill.set_color4f(rgba(WINE, 0.62), None);
            canvas.draw_rrect(chip_rr, &chip_fill);
            let mut chip_border = Paint::default();
            chip_border.set_anti_alias(true);
            chip_border.set_style(PaintStyle::Stroke);
            chip_border.set_stroke_width(1.0);
            chip_border.set_color4f(rgba(PEARL, 0.10), None);
            canvas.draw_rrect(chip_rr, &chip_border);

            let (_, cm) = chip_font.metrics();
            let cap = if cm.cap_height > 0.0 { cm.cap_height } else { 7.0 };
            let mut chip_text = Paint::default();
            chip_text.set_anti_alias(true);
            chip_text.set_color4f(
                if st.enabled {
                    rgba(CHAMP, 1.0)
                } else {
                    rgba(MAUVE, 1.0)
                },
                None,
            );
            draw_tracked_em(
                canvas,
                &label,
                (chip_x + chip_pad_x, chip_y + (chip_h + cap) * 0.5),
                &chip_font,
                &chip_text,
                0.18,
            );
        }

        // On/off toggle.
        draw_panel_toggle(canvas, row.toggle, st.enabled);

        // Settings affordance — a tiny "···" icon just left of the keybind
        // chip / toggle, only on modules that *have* sliders. Right-click the
        // row to open the popover (or left-click this dot to do the same).
        if !def.settings.is_empty() {
            let dot_cx = row.toggle.left
                - editor
                    .keybind_label_for(def.id)
                    .map(|l| {
                        9.0 * 2.0 + measure_tracked_em(&chip_font, &l, 0.18) + 10.0 + 16.0
                    })
                    .unwrap_or(16.0);
            let dot_cy = row.toggle.top + row.toggle.height() * 0.5;
            let mut dp = Paint::default();
            dp.set_anti_alias(true);
            dp.set_color4f(rgba(MAUVE, 0.85), None);
            for n in -1..=1 {
                canvas.draw_circle((dot_cx, dot_cy + n as f32 * 5.0), 1.4, &dp);
            }
        }
    }

    canvas.restore();

    // Scrollbar — a thin pearl track + rose knob on the right edge of the
    // panel, only drawn when content overflows.
    if max_scroll > 0.0 {
        let track_x = panel.right - 6.0;
        let track_top = body_top + 2.0;
        let track_bot = panel.bottom - 6.0;
        let track_h = (track_bot - track_top).max(20.0);
        let mut tp = Paint::default();
        tp.set_anti_alias(true);
        tp.set_color4f(rgba(PEARL, 0.06), None);
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(track_x, track_top, 2.0, track_h),
                1.0,
                1.0,
            ),
            &tp,
        );

        // Knob — proportional to the visible / content ratio.
        let visible = panel.bottom - body_top - 8.0;
        let knob_h = (track_h * (visible / content_h)).max(24.0);
        let knob_t = (scroll_y / max_scroll) * (track_h - knob_h);
        let mut kp = Paint::default();
        kp.set_anti_alias(true);
        kp.set_color4f(rgba(ROSE, 0.55), None);
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(track_x - 1.0, track_top + knob_t, 4.0, knob_h),
                2.0,
                2.0,
            ),
            &kp,
        );
    }
}

/// Draw one module-setting slider — a thin Velvet track with a pearl knob and
/// the current value. `enabled` dims it when the parent module is off.
pub(super) fn draw_module_slider(
    canvas: &Canvas,
    area: Rect,
    setting: &catalog::ModuleSetting,
    value: f32,
    enabled: bool,
    fonts: &FontStore,
) {
    let alpha = if enabled { 1.0 } else { 0.45 };
    let cy = area.top + area.height() * 0.5;
    let value_w = 54.0;
    let track_left = area.left + 14.0;
    let track_right = area.right - value_w;
    let track_h = 4.0;

    // Track.
    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85 * alpha), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h * 0.5, track_h * 0.5), &tp);

    let span = (setting.max - setting.min).max(0.001);
    let frac = ((value - setting.min) / span).clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);

    // Fill up to the knob — rose→lavender.
    if knob_x > track_left + 1.0 {
        let fill = Rect::from_xywh(track_left, cy - track_h * 0.5, knob_x - track_left, track_h);
        let mut fp = Paint::default();
        fp.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(track_left, cy), Point::new(track_right, cy)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rgba(ROSE, alpha), rgba(LAV, alpha)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fp.set_shader(shader);
        }
        canvas.draw_rrect(RRect::new_rect_xy(fill, track_h * 0.5, track_h * 0.5), &fp);
    }

    // Knob.
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, alpha), None);
    canvas.draw_circle((knob_x, cy), 6.0, &knob);

    // Value, right-aligned in the reserved strip.
    let val_font = fonts.jetbrains_mono(13.0);
    let val_str = format!("{}", value.round() as i32);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, alpha), None);
    let (_, m) = val_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    canvas.draw_str(&val_str, (track_right + 16.0, cy + cap * 0.5), &val_font, &vp);
}

/// Hit-rects for the module settings popover — the card itself, the close
/// button at top-right, and one rect per slider track. Right-click on a
/// MODULES row with sliders opens this popover; geometry must match
/// [`draw_module_popover`] one-to-one for hit-tests to line up.
pub(super) struct ModulePopoverLayout {
    pub(super) card: Rect,
    pub(super) close: Rect,
    pub(super) sliders: Vec<Rect>,
}

pub(super) fn module_popover_layout(w: f32, h: f32, idx: usize) -> ModulePopoverLayout {
    const CARD_W: f32 = 520.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 92.0; // eyebrow + name + (single-line description)
    const SLIDER_ROW_H: f32 = 60.0; // label + track per setting
    const FOOTER_H: f32 = 24.0; // bottom padding

    let setting_count = catalog::REGISTRY
        .get(idx)
        .map(|m| m.settings.len())
        .unwrap_or(0)
        .max(1);
    let card_h = PAD + HEADER_H + SLIDER_ROW_H * setting_count as f32 + FOOTER_H + PAD;
    let card_x = (w - CARD_W) * 0.5;
    let card_y = (h - card_h) * 0.5;
    let card = Rect::from_xywh(card_x, card_y, CARD_W, card_h);

    // Close button — a 28×28 hit-area in the top-right corner.
    let close = Rect::from_xywh(card.right - PAD - 22.0, card.top + PAD - 4.0, 28.0, 28.0);

    // Per-setting slider track rects. Each row reserves SLIDER_ROW_H of
    // vertical space; the track itself sits in the lower half so the label
    // can sit above it.
    let sliders: Vec<Rect> = (0..catalog::REGISTRY[idx].settings.len())
        .map(|s| {
            let row_top = card.top + PAD + HEADER_H + s as f32 * SLIDER_ROW_H;
            Rect::from_xywh(
                card.left + PAD,
                row_top + 28.0,
                CARD_W - PAD * 2.0,
                SLIDER_ROW_H - 28.0,
            )
        })
        .collect();

    ModulePopoverLayout { card, close, sliders }
}

/// Draw the per-module settings popover — a centred glass card with one row
/// per slider. Backdrop dims the rest of the screen so the popover reads as
/// a focused modal.
pub(super) fn draw_module_popover(
    canvas: &Canvas,
    editor: &Editor,
    idx: usize,
    fonts: &FontStore,
    w: f32,
    h: f32,
) {
    let Some(def) = catalog::REGISTRY.get(idx) else {
        return;
    };
    let st = editor.modules.get(idx);
    let layout = module_popover_layout(w, h, idx);

    // ── Dim backdrop — black 0.45 over everything ─────────────────────────
    let mut dim = Paint::default();
    dim.set_anti_alias(true);
    dim.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.45), None);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &dim);

    // ── Card chrome — drop shadow + wine fill + rose-tinted border ────────
    let card_rr = RRect::new_rect_xy(layout.card, 16.0, 16.0);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.6), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 18.0, false));
    let shadow_rrect = RRect::new_rect_xy(
        layout.card.with_offset((0.0, 10.0)),
        16.0,
        16.0,
    );
    canvas.draw_rrect(shadow_rrect, &shadow);

    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.92), None);
    canvas.draw_rrect(card_rr, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.28), None);
    canvas.draw_rrect(card_rr, &border);

    // Inset top-edge highlight.
    canvas.save();
    let top_clip = Rect::from_xywh(layout.card.left, layout.card.top, layout.card.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(
        layout.card.left + 0.5,
        layout.card.top + 0.5,
        layout.card.right - 0.5,
        layout.card.bottom - 0.5,
    );
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, 15.5, 15.5), &top);
    canvas.restore();

    let pad = 24.0;
    let left = layout.card.left + pad;

    // ── Header — category eyebrow + module name + description ─────────────
    let cat_font = fonts.jetbrains_mono(10.0);
    let mut cat_paint = Paint::default();
    cat_paint.set_anti_alias(true);
    cat_paint.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        def.category.label(),
        (left, layout.card.top + pad + 14.0),
        &cat_font,
        &cat_paint,
        0.24,
    );

    let name_font = fonts.fraunces_axes(24.0, 100.0, 1.0, 540.0, Some(36.0));
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(def.name, (left, layout.card.top + pad + 44.0), &name_font, &name_paint);

    let desc_font = fonts.newsreader(13.0);
    let mut desc_paint = Paint::default();
    desc_paint.set_anti_alias(true);
    desc_paint.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.save();
    let desc_clip = Rect::new(
        left,
        layout.card.top + pad + 56.0,
        layout.card.right - pad,
        layout.card.top + pad + 80.0,
    );
    canvas.clip_rect(desc_clip, Some(ClipOp::Intersect), Some(true));
    canvas.draw_str(
        def.description,
        (left, layout.card.top + pad + 74.0),
        &desc_font,
        &desc_paint,
    );
    canvas.restore();

    // ── Close button (top-right) ──────────────────────────────────────────
    let cx = layout.close.left + layout.close.width() * 0.5;
    let cy = layout.close.top + layout.close.height() * 0.5;
    let mut close_paint = Paint::default();
    close_paint.set_anti_alias(true);
    close_paint.set_style(PaintStyle::Stroke);
    close_paint.set_stroke_width(1.5);
    close_paint.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.draw_line((cx - 6.0, cy - 6.0), (cx + 6.0, cy + 6.0), &close_paint);
    canvas.draw_line((cx + 6.0, cy - 6.0), (cx - 6.0, cy + 6.0), &close_paint);

    // ── One row per setting — label above the track, value to the right ───
    let label_font = fonts.newsreader(14.0);
    let val_font = fonts.jetbrains_mono(12.0);
    for (slot, setting) in def.settings.iter().enumerate() {
        let track = layout.sliders[slot];

        // Label (left).
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(setting.label, (left, track.top - 8.0), &label_font, &lp);

        // Value (right) — formatted with step-aware precision.
        let value = st.settings[slot];
        let val_str = if setting.step >= 1.0 {
            format!("{}", value.round() as i32)
        } else if setting.step >= 0.1 {
            format!("{:.1}", value)
        } else {
            format!("{:.2}", value)
        };
        let mut vp = Paint::default();
        vp.set_anti_alias(true);
        vp.set_color4f(rgba(CHAMP, 1.0), None);
        let (vw, _) = val_font.measure_str(&val_str, Some(&vp));
        canvas.draw_str(
            &val_str,
            (layout.card.right - pad - vw, track.top - 8.0),
            &val_font,
            &vp,
        );

        // The slider track itself reuses the existing `draw_module_slider`,
        // which lays out a thin pill + pearl knob.
        draw_module_slider(canvas, track, setting, value, true, fonts);
    }
}

// ────────────────────────────────────────────────────────────────────────
// CROSSHAIR editor — vanilla-replace toggle, true-to-life preview row,
// shape sliders, HSV colour picker (SV square + Hue strip + Alpha strip +
// hex readout). Layout is computed once per frame from `crosshair_layout`
// so render + hit-test share rects.
// ────────────────────────────────────────────────────────────────────────
