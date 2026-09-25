use super::*;

/// Draw the editor: a faint tint, a drag outline around each visible widget
/// (the hovered/dragged one highlighted + labelled), and a hint line.
pub(super) fn draw_editor(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    // Alignment guides — drawn while a drag is snapped to another widget.
    let mut guide = Paint::default();
    guide.set_anti_alias(true);
    guide.set_style(PaintStyle::Stroke);
    guide.set_stroke_width(1.0);
    guide.set_color4f(rgba(ROSE, 0.5), None);
    if let Some(sx) = editor.snap_x {
        canvas.draw_line((sx, 0.0), (sx, h), &guide);
    }
    if let Some(sy) = editor.snap_y {
        canvas.draw_line((0.0, sy), (w, sy), &guide);
    }

    // Widget outlines — the hovered/dragged or panel-selected one is lit.
    let active = editor.active_widget();
    for id in WidgetId::ALL {
        let b = editor.bounds[id.index()];
        if b.width() <= 0.0 {
            continue;
        }
        // Every widget keeps its dim outline (that is the "these are
        // draggable" affordance); the hovered or selected one is lit. The
        // resize grip is drawn only on the *selected* one, which is also the
        // only widget `editor_press` will resize — a grip on a merely-hovered
        // widget would be a target you could see but not hit.
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

    draw_side_panel(canvas, editor, fonts, h);
}

/// A drag outline around one widget — a rose rounded-rect; the active widget
/// gets a brighter ring and a name label above it.
pub(super) fn draw_widget_outline(
    canvas: &Canvas,
    bounds: Rect,
    active: bool,
    fonts: &FontStore,
    title: &str,
    anchor: Anchor,
    scale: f32,
    // Draw the resize grip. Only the selected widget can be resized, so only
    // it gets one.
    grip: bool,
) {
    let pad = 4.0;
    let outline = Rect::from_xywh(
        bounds.left - pad,
        bounds.top - pad,
        bounds.width() + pad * 2.0,
        bounds.height() + pad * 2.0,
    );
    let rrect = RRect::new_rect_xy(outline, 8.0, 8.0);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    if active {
        stroke.set_stroke_width(2.0);
        stroke.set_color4f(rgba(ROSE, 0.95), None);
    } else {
        stroke.set_stroke_width(1.5);
        stroke.set_color4f(rgba(ROSE, 0.4), None);
    }
    canvas.draw_rrect(rrect, &stroke);

    if active {
        let label_font = fonts.jetbrains_mono(11.0);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color4f(rgba(ROSE, 1.0), None);
        // Title, plus the size multiplier once it is no longer 1× — so the
        // user can see what they dragged it to and get back to default.
        let title = if (scale - 1.0).abs() > 0.005 {
            format!("{title}  ·  {scale:.2}×")
        } else {
            title.to_string()
        };
        draw_tracked_em(
            canvas,
            &title,
            (outline.left + 2.0, outline.top - 7.0),
            &label_font,
            &label_paint,
            0.12,
        );

    }

    if grip {
        // Resize grip, at the corner furthest from the anchor. Three stacked
        // diagonal ticks — the conventional "drag me" affordance, and legible
        // at 14px where an icon would not be.
        let grip = resize_handle_rect(bounds, anchor);
        let mut plate = Paint::default();
        plate.set_anti_alias(true);
        plate.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(RRect::new_rect_xy(grip, 4.0, 4.0), &plate);
        let mut edge = Paint::default();
        edge.set_anti_alias(true);
        edge.set_style(PaintStyle::Stroke);
        edge.set_stroke_width(1.0);
        edge.set_color4f(rgba(ROSE, 0.95), None);
        canvas.draw_rrect(RRect::new_rect_xy(grip, 4.0, 4.0), &edge);

        let mut tick = Paint::default();
        tick.set_anti_alias(true);
        tick.set_style(PaintStyle::Stroke);
        tick.set_stroke_width(1.2);
        tick.set_color4f(rgba(ROSE, 0.95), None);
        for i in 0..3 {
            let o = 3.0 + i as f32 * 3.5;
            canvas.draw_line(
                (grip.left + o, grip.bottom - 3.0),
                (grip.right - 3.0, grip.top + o),
                &tick,
            );
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Editor side panel — widget toggles + the 9-anchor preset grid.
// ────────────────────────────────────────────────────────────────────────

/// The 9 anchor presets the grid offers — an anchor plus the fractional
/// corner/edge/centre it sends the widget to.
pub(super) const ANCHOR_PRESETS: [(Anchor, f32, f32); 9] = [
    (Anchor::Tl, 0.02, 0.03),
    (Anchor::Tc, 0.50, 0.03),
    (Anchor::Tr, 0.98, 0.03),
    (Anchor::Ml, 0.02, 0.50),
    (Anchor::Mc, 0.50, 0.50),
    (Anchor::Mr, 0.98, 0.50),
    (Anchor::Bl, 0.02, 0.97),
    (Anchor::Bc, 0.50, 0.97),
    (Anchor::Br, 0.98, 0.97),
];

/// Hit-rects of the editor side panel, computed from the window height.
pub(super) struct PanelLayout {
    pub(super) panel: Rect,
    pub(super) rows: [Rect; WIDGET_COUNT],
    pub(super) toggles: [Rect; WIDGET_COUNT],
    pub(super) cells: [Rect; 9],
}

/// Lay out the left-edge editor panel — chip, widget rows + toggles, anchor
/// grid. Deterministic in the window height, so the renderer and the
/// hit-tester agree.
pub(super) fn panel_layout(h: f32) -> PanelLayout {
    const PANEL_W: f32 = 234.0;
    const PANEL_X: f32 = 20.0;
    const PAD: f32 = 18.0;
    const HEADER_H: f32 = 30.0; // eyebrow + gap
    const WLABEL_H: f32 = 22.0; // "WIDGETS" label + gap
    const ROW_H: f32 = 30.0;
    const SECTION_GAP: f32 = 20.0;
    const ALABEL_H: f32 = 22.0; // "ANCHOR" label + gap
    const CELL: f32 = 42.0;
    const CELL_GAP: f32 = 6.0;

    let grid_h = CELL * 3.0 + CELL_GAP * 2.0;
    let row_count = WidgetId::ALL.len() as f32;
    let panel_h =
        PAD * 2.0 + HEADER_H + WLABEL_H + ROW_H * row_count + SECTION_GAP + ALABEL_H + grid_h;
    let panel_y = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(PANEL_X, panel_y, PANEL_W, panel_h);

    let content_x = PANEL_X + PAD;
    let content_w = PANEL_W - PAD * 2.0;

    let rows_top = panel_y + PAD + HEADER_H + WLABEL_H;
    let mut rows = [empty_rect(); WIDGET_COUNT];
    let mut toggles = [empty_rect(); WIDGET_COUNT];
    for i in 0..WidgetId::ALL.len() {
        let row = Rect::from_xywh(content_x, rows_top + i as f32 * ROW_H, content_w, ROW_H);
        rows[i] = row;
        let tw = 34.0;
        let th = 18.0;
        toggles[i] = Rect::from_xywh(row.right - tw, row.top + (ROW_H - th) * 0.5, tw, th);
    }

    let grid_top = rows_top + ROW_H * row_count + SECTION_GAP + ALABEL_H;
    let grid_w = CELL * 3.0 + CELL_GAP * 2.0;
    let grid_x = content_x + (content_w - grid_w) * 0.5;
    let mut cells = [empty_rect(); 9];
    for r in 0..3 {
        for c in 0..3 {
            cells[r * 3 + c] = Rect::from_xywh(
                grid_x + c as f32 * (CELL + CELL_GAP),
                grid_top + r as f32 * (CELL + CELL_GAP),
                CELL,
                CELL,
            );
        }
    }

    PanelLayout {
        panel,
        rows,
        toggles,
        cells,
    }
}

/// Draw the editor side panel: a widget list (name + enable toggle) and a
/// 3×3 anchor preset grid for the selected widget.
pub(super) fn draw_side_panel(canvas: &Canvas, editor: &Editor, fonts: &FontStore, h: f32) {
    let pl = panel_layout(h);
    draw_chip(canvas, pl.panel, 14.0);

    let pad = 18.0;
    let left = pl.panel.left + pad;

    // Eyebrow.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "HUD EDITOR",
        (left, pl.panel.top + pad + 4.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    // Section labels.
    let label_font = fonts.jetbrains_mono(10.0);
    let mut label = Paint::default();
    label.set_anti_alias(true);
    label.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "WIDGETS",
        (left, pl.rows[0].top - 8.0),
        &label_font,
        &label,
        0.18,
    );
    draw_tracked_em(
        canvas,
        "ANCHOR",
        (left, pl.cells[0].top - 8.0),
        &label_font,
        &label,
        0.18,
    );

    // Widget rows.
    let name_font = fonts.jetbrains_mono(12.0);
    let (_, nm) = name_font.metrics();
    let ncap = if nm.cap_height > 0.0 { nm.cap_height } else { 9.0 };
    for (i, id) in WidgetId::ALL.into_iter().enumerate() {
        let row = pl.rows[i];
        let wl = editor.layout.get(id);
        let mid_y = row.top + row.height() * 0.5;

        if editor.selected == Some(id) {
            let mut hl = Paint::default();
            hl.set_anti_alias(true);
            hl.set_color4f(rgba(ROSE, 0.14), None);
            canvas.draw_rrect(RRect::new_rect_xy(row, 7.0, 7.0), &hl);
        }

        // Enabled dot.
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            if wl.enabled { rgba(ROSE, 1.0) } else { rgba(MAUVE, 0.4) },
            None,
        );
        canvas.draw_circle((row.left + 9.0, mid_y), 3.5, &dot);

        // Name.
        let mut name = Paint::default();
        name.set_anti_alias(true);
        name.set_color4f(
            if wl.enabled { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.7) },
            None,
        );
        draw_tracked_em(
            canvas,
            id.title(),
            (row.left + 24.0, mid_y + ncap * 0.5),
            &name_font,
            &name,
            0.06,
        );

        draw_panel_toggle(canvas, pl.toggles[i], wl.enabled);
    }

    // Anchor preset grid — highlights the selected widget's current anchor.
    let has_selection = editor.selected.is_some();
    let selected_anchor = editor.selected.map(|s| editor.layout.get(s).anchor);
    for (i, &(cell_anchor, _, _)) in ANCHOR_PRESETS.iter().enumerate() {
        let current = selected_anchor == Some(cell_anchor);
        draw_anchor_cell(canvas, pl.cells[i], cell_anchor, current, has_selection);
    }
}

/// A small on/off pill toggle for a widget row.
pub(super) fn draw_panel_toggle(canvas: &Canvas, rect: Rect, on: bool) {
    let r = rect.height() * 0.5;
    let rrect = RRect::new_rect_xy(rect, r, r);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(ROSE, 0.6) } else { rgba(WINE, 0.85) }, None);
    canvas.draw_rrect(rrect, &bg);
    if !on {
        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(rgba(ROSE, 0.25), None);
        canvas.draw_rrect(rrect, &border);
    }

    let knob_r = r - 3.0;
    let cx = if on {
        rect.right - knob_r - 3.0
    } else {
        rect.left + knob_r + 3.0
    };
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(if on { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.9) }, None);
    canvas.draw_circle((cx, rect.top + r), knob_r, &knob);
}

/// One cell of the anchor grid — a mini screen-position map: a dot sits where
/// the cell's anchor would pin the widget. The selected widget's current
/// anchor cell is highlighted.
pub(super) fn draw_anchor_cell(canvas: &Canvas, rect: Rect, anchor: Anchor, current: bool, active: bool) {
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        if current { rgba(ROSE, 0.28) } else { rgba(WINE, 0.7) },
        None,
    );
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        if current { rgba(ROSE, 0.8) } else { rgba(ROSE, 0.15) },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    // The position dot.
    let (fx, fy) = anchor.fractions();
    let inset = 10.0;
    let dx = rect.left + inset + (rect.width() - inset * 2.0) * fx;
    let dy = rect.top + inset + (rect.height() - inset * 2.0) * fy;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(
        if !active {
            rgba(MAUVE, 0.45)
        } else if current {
            rgba(PEARL, 1.0)
        } else {
            rgba(ROSE, 0.85)
        },
        None,
    );
    canvas.draw_circle((dx, dy), 3.0, &dot);
}

// ────────────────────────────────────────────────────────────────────────
// Overlay dashboard — the top-centre view-tab strip + the Settings view.
// ────────────────────────────────────────────────────────────────────────
