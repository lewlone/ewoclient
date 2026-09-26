use super::*;

/// Roman numeral for `n` (1-based level). Empty string for `n <= 0`.
pub(super) fn roman(n: i32) -> String {
    const TABLE: &[(i32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    if n <= 0 {
        return String::new();
    }
    let mut n = n;
    let mut s = String::new();
    for &(value, sym) in TABLE {
        while n >= value {
            s.push_str(sym);
            n -= value;
        }
    }
    s
}

/// Format a tick duration as `m:ss`. Negative durations are infinite effects.
pub(super) fn format_duration(ticks: i32) -> String {
    if ticks < 0 {
        return "∞".to_string();
    }
    let secs = ticks / 20;
    format!("{}:{:02}", secs / 60, secs % 60)
}

// ────────────────────────────────────────────────────────────────────────
// Widgets. Each takes an anchor + anchor point, draws, and returns its bounds.
// ────────────────────────────────────────────────────────────────────────

/// A stat chip — re-skin of `hud.jsx`'s `.hud-stat` (FPS / Ping): a Fraunces
/// number and a tracked JetBrains Mono unit on a wine chip.
pub(super) fn draw_stat(
    canvas: &Canvas,
    value: &str,
    unit: &str,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18; // tracked eyebrow — Velvet label idiom

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 8.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    // Size the chip to the number's cap height — digits and the uppercase
    // unit have no descenders, so capped sizing hugs the glyphs.
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    // Number cap-top at `y + pad_y`; the unit shares the baseline (CSS
    // `align-items: baseline`).
    let baseline_y = y + pad_y + cap;
    let num_x = x + pad_x;
    let unit_x = num_x + num_w + gap;

    // Number — Fraunces, with the in-world 3-stop dark velvet shadow stack so
    // glyphs hold against snow, lava, and dark caves alike.
    draw_iw_text_shadow(canvas, value, (num_x, baseline_y), &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(value, (num_x, baseline_y), &num_font, &num_paint);

    // Unit — tracked JetBrains Mono eyebrow, rose to mirror the design's
    // `.w-fps .lbl` accent color.
    draw_iw_text_shadow_tracked(
        canvas,
        unit,
        (unit_x, baseline_y),
        &unit_font,
        unit_tracking_em,
    );
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(ROSE, 1.0), None);
    draw_tracked_em(
        canvas,
        unit,
        (unit_x, baseline_y),
        &unit_font,
        &unit_paint,
        unit_tracking_em,
    );

    chip
}

/// Coords chip — re-skin of `hud.jsx`'s `coords` element: a tracked rose "XYZ"
/// label and the player position in pearl JetBrains Mono.
pub(super) fn draw_coords(
    canvas: &Canvas,
    x: f64,
    y: f64,
    z: f64,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let label = "XYZ";
    // x/z to one decimal, y rounded — matches the prototype's `-128.4 64 -1492.0`.
    let value = format!("{:.1}  {}  {:.1}", x, y.round() as i64, z);

    let label_font = fonts.jetbrains_mono(12.0);
    // Fraunces 18 SOFT 30 opsz 36 — design's `.w-coords .v` calibration.
    let value_font = fonts.fraunces_axes(18.0, 30.0, 0.0, 500.0, Some(36.0));
    let label_tracking_em = 0.22; // CSS `letter-spacing: .22em` per the redesign.

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 14.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
    let (value_w, _) = value_font.measure_str(&value, Some(&probe));

    let (_, m) = value_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 18.0 * 0.72 };
    let chip_w = pad_x * 2.0 + label_w + gap + value_w;
    let chip_h = pad_y * 2.0 + cap;
    let (cx, cy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(cx, cy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = cy + pad_y + cap;

    // "XYZ" — tracked rose label with the in-world shadow stack.
    draw_iw_text_shadow_tracked(
        canvas,
        label,
        (cx + pad_x, baseline_y),
        &label_font,
        label_tracking_em,
    );
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        label,
        (cx + pad_x, baseline_y),
        &label_font,
        &label_paint,
        label_tracking_em,
    );

    // Position — pearl Fraunces with the in-world shadow stack.
    let value_pos = (cx + pad_x + label_w + gap, baseline_y);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

    chip
}

/// One key-cap glyph.
pub(super) enum KeyGlyph {
    Letter(&'static str),
    Bar, // the space bar
}

/// Draw one WASD/space key cap — re-skin of `hud.jsx`'s `.hud-key`.
pub(super) fn draw_key(canvas: &Canvas, rect: Rect, active: bool, fonts: &FontStore, glyph: KeyGlyph) {
    let radius = 9.0;
    // Active keys press down 2px (CSS `.hud-key.active { transform: translateY(2px) }`).
    let rect = if active {
        rect.with_offset((0.0, 2.0))
    } else {
        rect
    };
    let rrect = RRect::new_rect_xy(rect, radius, radius);

    if active {
        // Rose glow behind the cap (CSS `box-shadow: 0 0 20px`).
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(ROSE, 0.5), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 10.0, false));
        canvas.draw_rrect(rrect, &glow);

        // 135° rose→lavender gradient fill.
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.right, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rgba(ROSE, 0.55), rgba(LAV, 0.45)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        }
        canvas.draw_rrect(rrect, &fill);

        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.5);
        border.set_color4f(rgba(ROSE, 0.7), None);
        canvas.draw_rrect(rrect, &border);
    } else {
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(rgba(WINE, 0.55), None);
        canvas.draw_rrect(rrect, &fill);

        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.5);
        border.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.15), None);
        canvas.draw_rrect(rrect, &border);
    }

    let glyph_color = if active {
        rgba(PEARL, 1.0)
    } else {
        rgba(MAUVE, 1.0)
    };
    let cx = rect.left + rect.width() / 2.0;
    let cy = rect.top + rect.height() / 2.0;
    match glyph {
        KeyGlyph::Letter(s) => {
            let font = fonts.jetbrains_mono(20.0);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color4f(glyph_color, None);
            let (lw, _) = font.measure_str(s, Some(&p));
            let (_, m) = font.metrics();
            let cap = if m.cap_height > 0.0 { m.cap_height } else { 14.0 };
            canvas.draw_str(s, (cx - lw / 2.0, cy + cap / 2.0), &font, &p);
        }
        KeyGlyph::Bar => {
            // The space bar — a short rounded underscore.
            let bw = 18.0;
            let bh = 3.0;
            let bar = Rect::from_xywh(cx - bw / 2.0, cy - bh / 2.0, bw, bh);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color4f(glyph_color, None);
            canvas.draw_rrect(RRect::new_rect_xy(bar, bh / 2.0, bh / 2.0), &p);
        }
    }
}

/// Keystrokes widget — re-skin of `hud.jsx`'s `keystrokes` element: a WASD
/// cross with a space bar below, lit as the keys are held.
pub(super) fn draw_keystrokes(
    canvas: &Canvas,
    keys: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const KEY: f32 = 36.0;
    const GAP: f32 = 4.0;
    let grid_w = KEY * 3.0 + GAP * 2.0;
    let row3_h = KEY * 0.7;
    let grid_h = KEY + GAP + KEY + GAP + row3_h;
    let (ox, oy) = anchor.origin(ax, ay, grid_w, grid_h);

    let col = |c: f32| ox + c * (KEY + GAP);
    let row2 = oy + KEY + GAP;
    let row3 = row2 + KEY + GAP;

    // Keys bitmask — mirror of `EwoHudData`'s bit layout.
    const FWD: i32 = 1;
    const LEFT: i32 = 1 << 1;
    const BACK: i32 = 1 << 2;
    const RIGHT: i32 = 1 << 3;
    const JUMP: i32 = 1 << 4;

    draw_key(
        canvas,
        Rect::from_xywh(col(1.0), oy, KEY, KEY),
        keys & FWD != 0,
        fonts,
        KeyGlyph::Letter("W"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(0.0), row2, KEY, KEY),
        keys & LEFT != 0,
        fonts,
        KeyGlyph::Letter("A"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(1.0), row2, KEY, KEY),
        keys & BACK != 0,
        fonts,
        KeyGlyph::Letter("S"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(2.0), row2, KEY, KEY),
        keys & RIGHT != 0,
        fonts,
        KeyGlyph::Letter("D"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(ox, row3, grid_w, row3_h),
        keys & JUMP != 0,
        fonts,
        KeyGlyph::Bar,
    );

    Rect::from_xywh(ox, oy, grid_w, grid_h)
}

/// Armor widget — re-skin of `hud.jsx`'s `armor` element: four durability
/// gauges (head/chest/legs/feet), each a dark slot with a rose→lavender fill
/// rising from the bottom and the percentage centred on it.
pub(super) fn draw_armor(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const SLOT_W: f32 = 50.0;
    const SLOT_H: f32 = 44.0;
    const GAP: f32 = 6.0;
    const PAD: f32 = 6.0;
    let chip_w = PAD * 2.0 + SLOT_W * 4.0 + GAP * 3.0;
    let chip_h = PAD * 2.0 + SLOT_H;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, 12.0);

    let pct_font = fonts.jetbrains_mono(14.0);

    for i in 0..4 {
        let sx = ox + PAD + i as f32 * (SLOT_W + GAP);
        let sy = oy + PAD;
        let slot = Rect::from_xywh(sx, sy, SLOT_W, SLOT_H);
        let slot_rr = RRect::new_rect_xy(slot, 6.0, 6.0);

        // Recessed dark track.
        let mut track = Paint::default();
        track.set_anti_alias(true);
        track.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(slot_rr, &track);

        if !data.armor_present(i) {
            continue;
        }
        let durability = data.armor_durability(i).clamp(0.0, 1.0);
        let fill_h = SLOT_H * durability;
        if fill_h > 0.5 {
            let fill_top = sy + SLOT_H - fill_h;
            let fill = Rect::from_xywh(sx, fill_top, SLOT_W, fill_h);
            // Clip to the slot's rounded corners while the fill rect is square.
            canvas.save();
            canvas.clip_rrect(slot_rr, Some(ClipOp::Intersect), Some(true));
            let mut fill_paint = Paint::default();
            fill_paint.set_anti_alias(true);
            // 180° rose→lavender down the bar (CSS `.hud-armor-bar`).
            if let Some(shader) = gradient_shader::linear(
                (Point::new(sx, fill_top), Point::new(sx, sy + SLOT_H)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(LAV, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill_paint.set_shader(shader);
            }
            canvas.draw_rect(fill, &fill_paint);
            canvas.restore();
        }

        // Percentage, centred on the slot, pearl with a hard shadow so it
        // reads over both the filled and empty parts of the gauge.
        let pct = format!("{}", (durability * 100.0).round() as i32);
        let mut probe = Paint::default();
        let (pw, _) = pct_font.measure_str(&pct, Some(&probe));
        let (_, pm) = pct_font.metrics();
        let pcap = if pm.cap_height > 0.0 { pm.cap_height } else { 10.0 };
        let px = sx + SLOT_W / 2.0 - pw / 2.0;
        let py = sy + SLOT_H / 2.0 + pcap / 2.0;

        probe.set_anti_alias(true);
        draw_iw_text_shadow(canvas, &pct, (px, py), &pct_font);
        let mut pct_paint = Paint::default();
        pct_paint.set_anti_alias(true);
        pct_paint.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(&pct, (px, py), &pct_font, &pct_paint);
    }

    chip
}

/// Potion widget — re-skin of `hud.jsx`'s `potion` element: a column of active
/// effects, each a colour-keyed icon with the effect name and remaining time.
pub(super) fn draw_potions(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let count = data.potion_count();
    if count == 0 {
        return empty_rect();
    }

    const ICON: f32 = 32.0;
    const ROW_GAP: f32 = 8.0;
    const PAD: f32 = 10.0;
    const ICON_GAP: f32 = 12.0;

    // Design `.w-potions .nm` — Fraunces 13 SOFT 60. Time — tracked rose mono 10.
    let name_font = fonts.fraunces_axes(13.0, 60.0, 0.0, 500.0, Some(24.0));
    let time_font = fonts.jetbrains_mono(10.0);
    let time_tracking_em = 0.18;

    // Decode every row and measure the widest, so the chip is content-sized.
    let mut rows: Vec<(String, String, i32)> = Vec::with_capacity(count);
    let mut text_w = 0.0_f32;
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    for i in 0..count {
        let p = data.potion(i);
        let name_line = if p.amplifier > 0 {
            format!("{} {}", p.name, roman(p.amplifier + 1))
        } else {
            p.name.clone()
        };
        let time_line = format_duration(p.duration);
        let (nw, _) = name_font.measure_str(&name_line, Some(&probe));
        let tw = measure_tracked_em(&time_font, &time_line, time_tracking_em);
        text_w = text_w.max(nw).max(tw);
        rows.push((name_line, time_line, p.color));
    }

    let chip_w = PAD * 2.0 + ICON + ICON_GAP + text_w;
    let chip_h = PAD * 2.0 + count as f32 * ICON + (count as f32 - 1.0) * ROW_GAP;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, 12.0);

    for (i, (name, time, color)) in rows.iter().enumerate() {
        let ry = oy + PAD + i as f32 * (ICON + ROW_GAP);

        // Colour-keyed icon — a vertical gradient (lighter top, darker bottom)
        // from the effect's packed RGB.
        let icon = Rect::from_xywh(ox + PAD, ry, ICON, ICON);
        let icon_rr = RRect::new_rect_xy(icon, 8.0, 8.0);
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let mut icon_paint = Paint::default();
        icon_paint.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(icon.left, icon.top),
                Point::new(icon.left, icon.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new((r * 1.3).min(1.0), (g * 1.3).min(1.0), (b * 1.3).min(1.0), 1.0),
                    Color4f::new(r * 0.62, g * 0.62, b * 0.62, 1.0),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            icon_paint.set_shader(shader);
        }
        canvas.draw_rrect(icon_rr, &icon_paint);

        let text_x = ox + PAD + ICON + ICON_GAP;

        // Name — pearl Fraunces with the in-world shadow stack.
        let name_pos = (text_x, ry + 14.0);
        draw_iw_text_shadow(canvas, name, name_pos, &name_font);
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(name, name_pos, &name_font, &name_paint);

        // Remaining time — tracked rose mono per the redesign's `.w-potions .t`.
        let time_pos = (text_x, ry + 28.0);
        draw_iw_text_shadow_tracked(canvas, time, time_pos, &time_font, time_tracking_em);
        let mut time_paint = Paint::default();
        time_paint.set_anti_alias(true);
        time_paint.set_color4f(rgba(ROSE, 1.0), None);
        draw_tracked_em(canvas, time, time_pos, &time_font, &time_paint, time_tracking_em);
    }

    chip
}

/// TargetHUD — re-skin of `hud.jsx`'s `targethud` element: the looked-at
/// entity's initial avatar, name, distance and a health bar.
pub(super) fn draw_target(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let name = data.target_name();
    let distance = data.target_distance();
    let health = data.target_health();
    let max_health = data.target_max_health();
    let has_health = max_health > 0.0;

    const AV: f32 = 64.0;
    const PAD_X: f32 = 18.0;
    const PAD_Y: f32 = 14.0;
    const GAP: f32 = 16.0;

    // Bumped SOFT 80 + WONK 1 — pulls the name into the wonkier italic-ish
    // territory called for in the design's `.w-target .name`. True italic
    // Fraunces would need a separate typeface load; this is the closest match
    // with the axes we expose today.
    let name_font = fonts.fraunces_axes(21.0, 80.0, 1.0, 600.0, Some(36.0));
    let dist_font = fonts.jetbrains_mono(13.0);
    let avatar_font = fonts.fraunces_axes(30.0, 40.0, 1.0, 700.0, None);

    let dist_str = format!("{:.1}m", distance);

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (name_w, _) = name_font.measure_str(&name, Some(&probe));
    let (dist_w, _) = dist_font.measure_str(&dist_str, Some(&probe));
    let meta_w = (name_w + 16.0 + dist_w).max(190.0); // min width keeps the bar usable

    let chip_w = PAD_X * 2.0 + AV + GAP + meta_w;
    let chip_h = PAD_Y * 2.0 + AV;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, 16.0);

    // ── Avatar — a rose→lavender tile with the name's initial ──────────────
    let avatar = Rect::from_xywh(ox + PAD_X, oy + PAD_Y, AV, AV);
    let avatar_rr = RRect::new_rect_xy(avatar, 14.0, 14.0);

    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_color4f(rgba(ROSE, 0.5), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 10.0, false));
    canvas.draw_rrect(avatar_rr, &glow);

    let mut avatar_paint = Paint::default();
    avatar_paint.set_anti_alias(true);
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(avatar.left, avatar.top),
            Point::new(avatar.right, avatar.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[rgba(ROSE, 1.0), rgba(LAV, 1.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        avatar_paint.set_shader(shader);
    }
    canvas.draw_rrect(avatar_rr, &avatar_paint);

    // The target's actual face when the mod has exported a skin for *this*
    // name; the monogram otherwise. Mobs never have one, and a player's skin
    // arrives a moment after they are first looked at, so the monogram is the
    // steady-state fallback rather than an error case.
    if !draw_player_head(canvas, avatar_rr, avatar, &name) {
        let initial = name
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('?')
            .to_string();
        let mut initial_paint = Paint::default();
        initial_paint.set_anti_alias(true);
        initial_paint.set_color4f(rgba(PEARL, 1.0), None);
        let (iw, _) = avatar_font.measure_str(&initial, Some(&initial_paint));
        let (_, im) = avatar_font.metrics();
        let icap = if im.cap_height > 0.0 { im.cap_height } else { 22.0 };
        canvas.draw_str(
            &initial,
            (
                avatar.left + AV / 2.0 - iw / 2.0,
                avatar.top + AV / 2.0 + icap / 2.0,
            ),
            &avatar_font,
            &initial_paint,
        );
    }

    // ── Meta — name + distance row, then a health bar ──────────────────────
    let meta_x = ox + PAD_X + AV + GAP;
    let meta_right = ox + chip_w - PAD_X;
    let name_baseline = oy + PAD_Y + 22.0;

    let name_pos = (meta_x, name_baseline);
    draw_iw_text_shadow(canvas, &name, name_pos, &name_font);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&name, name_pos, &name_font, &name_paint);

    let dist_pos = (meta_right - dist_w, name_baseline);
    draw_iw_text_shadow(canvas, &dist_str, dist_pos, &dist_font);
    let mut dist_paint = Paint::default();
    dist_paint.set_anti_alias(true);
    dist_paint.set_color4f(rgba(ROSE, 0.9), None);
    canvas.draw_str(&dist_str, dist_pos, &dist_font, &dist_paint);

    if has_health {
        let bar_y = name_baseline + 12.0;
        let bar_h = 5.0;
        let bar = Rect::from_xywh(meta_x, bar_y, meta_right - meta_x, bar_h);
        let bar_rr = RRect::new_rect_xy(bar, bar_h / 2.0, bar_h / 2.0);

        let mut track = Paint::default();
        track.set_anti_alias(true);
        track.set_color4f(Color4f::new(PEARL.0 as f32 / 255.0, PEARL.1 as f32 / 255.0, PEARL.2 as f32 / 255.0, 0.12), None);
        canvas.draw_rrect(bar_rr, &track);

        let frac = (health / max_health).clamp(0.0, 1.0);
        let fill_w = bar.width() * frac;
        if fill_w > 1.0 {
            // Rose halo behind the fill — the redesign's `box-shadow:0 0 10px`
            // on `.w-target .hp .bar::before`.
            let halo_rect = Rect::from_xywh(bar.left, bar_y, fill_w, bar_h);
            let halo_rr = RRect::new_rect_xy(halo_rect, bar_h / 2.0, bar_h / 2.0);
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, 0.5), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
            canvas.draw_rrect(halo_rr, &halo);

            let fill = Rect::from_xywh(bar.left, bar_y, fill_w, bar_h);
            let mut fill_paint = Paint::default();
            fill_paint.set_anti_alias(true);
            // 90° rose→ember across the bar per the redesign — keeps the "right
            // end = danger" visual cue regardless of fill level.
            const EMBER: (u8, u8, u8) = (0xC9, 0x6A, 0x7A);
            if let Some(shader) = gradient_shader::linear(
                (Point::new(bar.left, bar_y), Point::new(bar.right, bar_y)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(EMBER, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill_paint.set_shader(shader);
            }
            canvas.draw_rrect(RRect::new_rect_xy(fill, bar_h / 2.0, bar_h / 2.0), &fill_paint);
        }
    }

    chip
}

// ────────────────────────────────────────────────────────────────────────
// PvP Utils widgets — jump-reset indicator + hit-range chip.
// ────────────────────────────────────────────────────────────────────────

/// Tier → display colour. Velvet palette (matches `EwoPvpConfig` defaults on
/// the Java side). The matched-zone colour for hit-range comes from the wire,
/// not this table.
pub(super) fn pvp_tier_color(tier: PvpTier) -> (u8, u8, u8) {
    match tier {
        PvpTier::Perfect => CHAMP,
        PvpTier::SlightlyLate => ROSE,
        PvpTier::Late => (0xC9, 0x6A, 0x7A), // --accent-ember
        PvpTier::SlightlyEarly => LAV,
        PvpTier::Early => BERRY,
        PvpTier::None => MAUVE,
    }
}

/// "PERFECT RESET" / "+50 ms LATE" / etc. — the Velvet re-skin of the
/// source mod's JumpResetHud. A wine chip with the tier label in Fraunces and
/// a tracked-mono "ms" suffix; on PERFECT, an extra rose-champagne glow.
pub(super) fn draw_jump_reset_text(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let tier = if data.pvp_jump_active() { data.pvp_jump_tier() } else { PvpTier::Perfect };
    let offset_ms = data.pvp_jump_offset_ms();
    let fade = if data.pvp_jump_active() { data.pvp_jump_fade() } else { 0.45 };

    let tier_text = tier.label();
    let ms_text = match tier {
        PvpTier::Perfect | PvpTier::None => String::new(),
        _ => {
            let sign = if offset_ms >= 0 { "+" } else { "" };
            format!("  {}{} ms", sign, offset_ms)
        }
    };

    let title_font = fonts.fraunces_axes(22.0, 40.0, 0.0, 600.0, None);
    let ms_font = fonts.jetbrains_mono(13.0);
    let ms_tracking_em = 0.14;

    let pad_x = 18.0;
    let pad_y = 10.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (title_w, _) = title_font.measure_str(tier_text, Some(&probe));
    let ms_w = if ms_text.is_empty() { 0.0 } else { measure_tracked_em(&ms_font, &ms_text, ms_tracking_em) };

    let (_, m) = title_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 22.0 * 0.72 };
    let chip_w = pad_x * 2.0 + title_w + ms_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let tier_color = pvp_tier_color(tier);
    let alpha = if data.pvp_jump_active() { fade.clamp(0.0, 1.0) } else { 0.5 };

    // Glow under PERFECT — celebratory champagne halo, only when the result
    // is fresh (no glow in the editor preview).
    if tier == PvpTier::Perfect && data.pvp_jump_active() {
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(tier_color, 0.55 * fade), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 14.0, false));
        canvas.draw_str(tier_text, (x + pad_x, baseline_y), &title_font, &glow);
    }

    // In-world 3-stop dark velvet text shadow for legibility over any backdrop.
    let title_pos = (x + pad_x, baseline_y);
    draw_iw_text_shadow(canvas, tier_text, title_pos, &title_font);

    // The tier label itself.
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(rgba(tier_color, alpha), None);
    canvas.draw_str(tier_text, title_pos, &title_font, &title_paint);

    // "+50 ms LATE" suffix — tracked mono in mauve so the tier word reads first.
    if !ms_text.is_empty() {
        let ms_pos = (x + pad_x + title_w, baseline_y);
        draw_iw_text_shadow_tracked(canvas, &ms_text, ms_pos, &ms_font, ms_tracking_em);
        let mut ms_paint = Paint::default();
        ms_paint.set_anti_alias(true);
        ms_paint.set_color4f(rgba(MAUVE, alpha), None);
        draw_tracked_em(canvas, &ms_text, ms_pos, &ms_font, &ms_paint, ms_tracking_em);
    }

    chip
}

/// The timing meter — a Velvet glass pill with a centre rose-pip "perfect"
/// marker and a sliding pearl tick at the player's actual offset. Replaces
/// the source mod's red→green→red boss-bar with the Velvet language.
pub(super) fn draw_jump_reset_bar(
    canvas: &Canvas,
    data: &HudData,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const BAR_W: f32 = 180.0;
    const BAR_H: f32 = 6.0;
    const PIP: f32 = 12.0; // tall side: the offset marker height
    let chip_h = PIP.max(BAR_H) + 6.0;
    let (x, y) = anchor.origin(ax, ay, BAR_W, chip_h);
    let bounds = Rect::from_xywh(x, y, BAR_W, chip_h);

    let active = data.pvp_jump_active();
    let fade = if active { data.pvp_jump_fade() } else { 0.5 };
    let tier = if active { data.pvp_jump_tier() } else { PvpTier::Perfect };
    let offset_ms = if active { data.pvp_jump_offset_ms() as f32 } else { 0.0 };

    let track = Rect::from_xywh(x, y + (chip_h - BAR_H) * 0.5, BAR_W, BAR_H);
    let track_rr = RRect::new_rect_xy(track, BAR_H * 0.5, BAR_H * 0.5);

    // Track — a thin wine pill with a faint rose hairline (matches HUD chip).
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(rgba(WINE, 0.62), None);
    canvas.draw_rrect(track_rr, &bg);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.18), None);
    canvas.draw_rrect(track_rr, &border);

    // The "perfect" marker — a centre pip in champagne with a soft halo.
    let cx = x + BAR_W * 0.5;
    let center_top = y + (chip_h - PIP) * 0.5;
    let center_rect = Rect::from_xywh(cx - 1.0, center_top, 2.0, PIP);
    let mut center_paint = Paint::default();
    center_paint.set_anti_alias(true);
    center_paint.set_color4f(rgba(CHAMP, 0.55), None);
    canvas.draw_rect(center_rect, &center_paint);

    // The "your offset" mark — a small pearl tick slid out to the right
    // (late) or left (early) by an offset proportional to ±300 ms full-scale.
    if active {
        const MAX_MS: f32 = 300.0;
        let norm = (offset_ms / MAX_MS).clamp(-1.0, 1.0);
        let mx = cx + norm * (BAR_W * 0.5 - 4.0);
        let tier_color = pvp_tier_color(tier);
        let mark_w = 3.0;
        let mark_h = PIP + 2.0;
        let mark = Rect::from_xywh(mx - mark_w * 0.5, y + (chip_h - mark_h) * 0.5, mark_w, mark_h);
        let mark_rr = RRect::new_rect_xy(mark, 1.5, 1.5);

        // Halo behind the mark.
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(rgba(tier_color, 0.55 * fade), None);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, false));
        canvas.draw_rrect(mark_rr, &halo);

        let mut mark_paint = Paint::default();
        mark_paint.set_anti_alias(true);
        mark_paint.set_color4f(rgba(PEARL, fade), None);
        canvas.draw_rrect(mark_rr, &mark_paint);
    } else {
        // In editor preview, draw a dim mark at the centre so the widget has
        // visible chrome to grab.
        let mark = Rect::from_xywh(cx - 1.5, y + (chip_h - PIP) * 0.5 - 1.0, 3.0, PIP + 2.0);
        let mut mark_paint = Paint::default();
        mark_paint.set_anti_alias(true);
        mark_paint.set_color4f(rgba(PEARL, 0.35), None);
        canvas.draw_rrect(RRect::new_rect_xy(mark, 1.5, 1.5), &mark_paint);
    }

    bounds
}

/// Hit-range chip — a big Fraunces distance reading + a tracked "BLOCKS"
/// eyebrow, tinted by the matched zone's colour (set by the user in pvp.toml).
pub(super) fn draw_hit_range(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let active = data.pvp_hit_active();
    let distance = if active { data.pvp_hit_distance() } else { 3.00 };
    let fade = if active { data.pvp_hit_fade() } else { 0.45 };
    let zone_color = if active && data.pvp_hit_color() != 0 {
        let c = data.pvp_hit_color();
        (((c >> 16) as u8), ((c >> 8) as u8), (c as u8))
    } else {
        ROSE
    };

    let value = format!("{:.2}", distance.max(0.0));
    let unit = "BLOCKS";

    let num_font = fonts.fraunces_axes(28.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(12.0);
    let unit_tracking_em = 0.18;

    let pad_x = 16.0;
    let pad_y = 9.0;
    let gap = 10.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(&value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 28.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let alpha = if active { fade.clamp(0.0, 1.0) } else { 0.5 };

    // In-world 3-stop dark velvet shadow stack.
    let value_pos = (x + pad_x, baseline_y);
    draw_iw_text_shadow(canvas, &value, value_pos, &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(zone_color, alpha), None);
    canvas.draw_str(&value, value_pos, &num_font, &num_paint);

    // "BLOCKS" — tracked rose eyebrow per the redesign's `.w-pvp .hitrange .lbl`.
    let unit_pos = (x + pad_x + num_w + gap, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(ROSE, alpha), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    chip
}

/// Click-per-second chip — two Fraunces numbers (left | right mouse) with a
/// thin mauve divider and a tracked "CPS" eyebrow. Mirrors the AxolotlClient /
/// Lunar idiom; left number is always the LMB rate.
pub(super) fn draw_cps(
    canvas: &Canvas,
    left: i32,
    right: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let left_s = left.to_string();
    let right_s = right.to_string();
    let unit = "CPS";

    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let inner_gap = 10.0; // between number and divider
    let unit_gap = 10.0; // between right number and unit
    let div_w = 1.5;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (l_w, _) = num_font.measure_str(&left_s, Some(&probe));
    let (r_w, _) = num_font.measure_str(&right_s, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w =
        pad_x * 2.0 + l_w + inner_gap + div_w + inner_gap + r_w + unit_gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let r_x = x + pad_x + l_w + inner_gap + div_w + inner_gap;

    // In-world 3-stop dark velvet shadow on both numbers.
    let left_pos = (x + pad_x, baseline_y);
    let right_pos = (r_x, baseline_y);
    draw_iw_text_shadow(canvas, &left_s, left_pos, &num_font);
    draw_iw_text_shadow(canvas, &right_s, right_pos, &num_font);

    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&left_s, left_pos, &num_font, &num_paint);
    canvas.draw_str(&right_s, right_pos, &num_font, &num_paint);

    // Divider — thin mauve vertical line spanning the cap height.
    let mut div_paint = Paint::default();
    div_paint.set_anti_alias(true);
    div_paint.set_style(PaintStyle::Stroke);
    div_paint.set_stroke_width(div_w);
    div_paint.set_color4f(rgba(MAUVE, 0.55), None);
    let div_x = x + pad_x + l_w + inner_gap + div_w * 0.5;
    let div_top = baseline_y - cap + 2.0;
    let div_bot = baseline_y - 2.0;
    canvas.draw_line((div_x, div_top), (div_x, div_bot), &div_paint);

    let unit_pos = (r_x + r_w + unit_gap, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    chip
}

/// Item-counter strip — four side-by-side cells (pearls / arrows / totems /
/// gapples). Each cell pairs a tracked Mono mauve label with a Fraunces count
/// in the item's accent colour. Zero-count cells render dimmed so the chip
/// width is stable as the player picks items up or drops them.
pub(super) fn draw_item_counters(
    canvas: &Canvas,
    pearls: i32,
    arrows: i32,
    totems: i32,
    gapples: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let items: [(&str, i32, (u8, u8, u8)); 4] = [
        ("PRL", pearls, LAV),
        ("ARW", arrows, CHAMP),
        ("TOT", totems, ROSE),
        ("GAP", gapples, BERRY),
    ];

    let num_font = fonts.fraunces_axes(22.0, 32.0, 0.0, 600.0, None);
    let label_font = fonts.jetbrains_mono(11.0);
    let label_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let label_to_num_gap = 6.0;
    let item_gap = 16.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 22.0 * 0.72 };

    let mut widths: [(f32, f32); 4] = [(0.0, 0.0); 4];
    let mut total = 0.0;
    for i in 0..4 {
        let (label, n, _) = items[i];
        let num_s = n.to_string();
        let (num_w, _) = num_font.measure_str(&num_s, Some(&probe));
        let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
        widths[i] = (label_w, num_w);
        total += label_w + label_to_num_gap + num_w;
    }
    total += item_gap * 3.0;

    let chip_w = pad_x * 2.0 + total;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let mut cursor_x = x + pad_x;

    for i in 0..4 {
        let (label, n, color) = items[i];
        let (label_w, num_w) = widths[i];
        let alpha = if n > 0 { 1.0 } else { 0.35 };

        let label_pos = (cursor_x, baseline_y);
        draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color4f(rgba(MAUVE, alpha), None);
        draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);

        let num_x = cursor_x + label_w + label_to_num_gap;
        let num_s = n.to_string();
        let num_pos = (num_x, baseline_y);
        if n > 0 {
            draw_iw_text_shadow(canvas, &num_s, num_pos, &num_font);
        }

        let mut num_paint = Paint::default();
        num_paint.set_anti_alias(true);
        let nc = if n > 0 { color } else { PEARL };
        num_paint.set_color4f(rgba(nc, alpha), None);
        canvas.draw_str(&num_s, num_pos, &num_font, &num_paint);

        cursor_x = num_x + num_w + item_gap;
    }

    chip
}

/// Local-player shield cooldown bar — a wide rose-fill pill on a wine track
/// with a "SHIELD" eyebrow + seconds-remaining numeric. The fraction is
/// taken straight from `ItemCooldowns.getCooldownPercent`; the vanilla
/// disable is 5 s (5 × 20 ticks = 100), so we render `pct * 5.0` seconds.
pub(super) fn draw_shield_cooldown(
    canvas: &Canvas,
    pct: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let pct = pct.clamp(0.0, 1.0);
    let seconds_left = pct * 5.0;

    let label = "SHIELD";
    let value = format!("{:.1}s", seconds_left);

    let label_font = fonts.jetbrains_mono(11.0);
    let value_font = fonts.fraunces_axes(18.0, 32.0, 0.0, 600.0, None);
    let label_tracking_em = 0.20;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let bar_h = 4.0;
    let bar_gap = 8.0;
    let value_gap = 10.0;
    let radius = 12.0;
    let chip_w = 180.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (val_w, _) = value_font.measure_str(&value, Some(&probe));
    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);

    let (_, vm) = value_font.metrics();
    let value_cap = if vm.cap_height > 0.0 { vm.cap_height } else { 18.0 * 0.72 };
    let (_, lm) = label_font.metrics();
    let label_cap = if lm.cap_height > 0.0 { lm.cap_height } else { 11.0 * 0.72 };

    let header_h = value_cap.max(label_cap);
    let chip_h = pad_y * 2.0 + header_h + bar_gap + bar_h;

    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    // Header row — eyebrow on the left, seconds-remaining on the right.
    let baseline = y + pad_y + header_h;

    let label_pos = (x + pad_x, baseline);
    draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(CHAMP, 1.0), None);
    draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);

    let value_x = x + chip_w - pad_x - val_w;
    let _ = label_w + value_gap; // gap consumed implicitly by chip_w
    let value_pos = (value_x, baseline);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    let ember = (0xC9, 0x6A, 0x7A);
    value_paint.set_color4f(rgba(ember, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

    // Bar — track + fill below the header row.
    let bar_y = y + pad_y + header_h + bar_gap;
    let track = Rect::from_xywh(x + pad_x, bar_y, chip_w - pad_x * 2.0, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78), None);
    canvas.draw_rrect(track_rr, &track_paint);

    if pct > 0.0 {
        let fill_w = (chip_w - pad_x * 2.0) * pct;
        let fill_rect = Rect::from_xywh(x + pad_x, bar_y, fill_w, bar_h);
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        // Ember when high (just disabled), rose as it ticks down to ready.
        let color = if pct > 0.5 { ember } else { ROSE };
        fill.set_color4f(rgba(color, 1.0), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    chip
}

/// Combo counter chip — re-skin of `draw_stat` with an age-based alpha
/// fade. The count itself snaps to 0 server-side on timeout (5 s of no
/// hits) or hit-taken, but the visual fades smoothly over the last second
/// so a missed combo ages out instead of vanishing on the next frame.
pub(super) fn draw_combo(
    canvas: &Canvas,
    count: i32,
    age_secs: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    // Velvet fade: full alpha for the first 4 s, linear decay over the last
    // 1 s of the 5 s combo window. After that the count is 0 anyway.
    let alpha_mul = if age_secs < 4.0 {
        1.0
    } else if age_secs >= 5.0 {
        0.0
    } else {
        1.0 - (age_secs - 4.0)
    };

    let value = count.to_string();
    let unit = "COMBO";

    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 8.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(&value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);

    // Fade the entire widget — shell + text — together by wrapping the draw
    // calls in a save_layer with the alpha multiplier as its layer alpha.
    if alpha_mul <= 0.001 {
        return chip;
    }
    let mut layer_paint = Paint::default();
    layer_paint.set_anti_alias(true);
    layer_paint.set_alpha_f(alpha_mul);
    let layer_bounds = chip.with_outset((24.0, 24.0)); // include the iw_shell drop shadow
    canvas.save_layer(
        &skia_safe::canvas::SaveLayerRec::default()
            .bounds(&layer_bounds)
            .paint(&layer_paint),
    );

    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let num_x = x + pad_x;
    let unit_x = num_x + num_w + gap;

    // Number — champagne when combo is fresh (≥ 4 stacks reads as "real
    // combo"), otherwise rose.
    let num_color = if count >= 4 { CHAMP } else { ROSE };
    let num_pos = (num_x, baseline_y);
    draw_iw_text_shadow(canvas, &value, num_pos, &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(num_color, 1.0), None);
    canvas.draw_str(&value, num_pos, &num_font, &num_paint);

    let unit_pos = (unit_x, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    canvas.restore();

    chip
}

/// Attack-strength charge meter — a wide pill bar showing the vanilla
/// attack-strength scale (0..1). Fill colour ramps ember → rose → champagne
/// as the meter approaches full, so peripheral vision can tell at a glance
/// whether the next click will land a full-charge hit. A "READY" tracked-mono
/// eyebrow flashes briefly when charge crosses 0.98.
pub(super) fn draw_attack_charge(
    canvas: &Canvas,
    charge: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let charge = charge.clamp(0.0, 1.0);

    let label = "ATTACK";
    let label_font = fonts.jetbrains_mono(11.0);
    let label_tracking_em = 0.20;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let bar_h = 5.0;
    let bar_gap = 8.0;
    let radius = 12.0;
    let chip_w = 168.0;

    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
    let (_, lm) = label_font.metrics();
    let label_cap = if lm.cap_height > 0.0 { lm.cap_height } else { 11.0 * 0.72 };

    // Right-aligned percentage value in JetBrains Mono.
    let value = if charge >= 0.999 { "READY".to_string() } else { format!("{:>3}%", (charge * 100.0).round() as i32) };
    let value_font = fonts.jetbrains_mono(11.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (val_w, _) = value_font.measure_str(&value, Some(&probe));

    let header_h = label_cap;
    let chip_h = pad_y * 2.0 + header_h + bar_gap + bar_h;

    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline = y + pad_y + header_h;
    let label_pos = (x + pad_x, baseline);
    draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(CHAMP, 1.0), None);
    draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);
    let _ = label_w; // gap consumed implicitly by chip_w

    let value_pos = (x + chip_w - pad_x - val_w, baseline);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    let value_color = if charge >= 0.999 { CHAMP } else { PEARL };
    value_paint.set_color4f(rgba(value_color, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

    // Bar — track + fill below the header row.
    let bar_y = y + pad_y + header_h + bar_gap;
    let track = Rect::from_xywh(x + pad_x, bar_y, chip_w - pad_x * 2.0, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78), None);
    canvas.draw_rrect(track_rr, &track_paint);

    if charge > 0.01 {
        let fill_w = (chip_w - pad_x * 2.0) * charge;
        let fill_rect = Rect::from_xywh(x + pad_x, bar_y, fill_w, bar_h);
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        // ember when half-or-less, rose at moderate, champagne when ready.
        let ember = (0xC9, 0x6A, 0x7A);
        let color = if charge >= 0.999 {
            CHAMP
        } else if charge >= 0.66 {
            ROSE
        } else {
            ember
        };
        fill.set_color4f(rgba(color, 1.0), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    chip
}

/// Linear distance-fade alpha multiplier for world-anchored indicators.
/// 1.0 at distance ≤ near, falls off to `min` at distance ≥ far, linear in
/// between. Cheap declutter for busy fights — far entities dim, near ones
/// stay crisp.
pub(super) fn distance_fade_alpha(distance: f32, near: f32, far: f32, min: f32) -> f32 {
    if distance <= near {
        return 1.0;
    }
    if distance >= far {
        return min;
    }
    let t = (distance - near) / (far - near);
    1.0 - t * (1.0 - min)
}

/// Overhead totem-of-undying pop counter — a small rose chip with `× N`
/// painted just above the entity's head. Drawn only when `totem_count > 0`,
/// so entities without observed pops stay un-cluttered. Alpha fades with
/// distance so far entities dim out.
pub(super) fn draw_totem_overhead(canvas: &Canvas, ind: &Indicator, fonts: &FontStore) {
    let label = format!("\u{00D7} {}", ind.totem_count); // "× N"
    let num_font = fonts.fraunces_axes(14.0, 30.0, 0.0, 600.0, None);
    let pad_x = 7.0;
    let pad_y = 3.5;

    let alpha_mul = distance_fade_alpha(ind.distance, 6.0, 32.0, 0.35);

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (lw, _) = num_font.measure_str(&label, Some(&probe));
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 14.0 * 0.72 };
    let chip_w = pad_x * 2.0 + lw;
    let chip_h = pad_y * 2.0 + cap;

    // Stacked above the head; vertical-offset 22 px clears any nametag.
    let cx = ind.screen_x;
    let cy = ind.screen_y - 22.0;
    let x = cx - chip_w * 0.5;
    let y = cy - chip_h * 0.5;
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);

    // Wine fill + rose hairline — the Velvet chip language at small scale.
    let rrect = RRect::new_rect_xy(chip, chip_h * 0.45, chip_h * 0.45);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.78 * alpha_mul), None);
    canvas.draw_rrect(rrect, &fill);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.55 * alpha_mul), None);
    canvas.draw_rrect(rrect, &border);

    let baseline = y + pad_y + cap;
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55 * alpha_mul), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_str(&label, (x + pad_x, baseline + 1.0), &num_font, &shadow);

    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(ROSE, alpha_mul), None);
    canvas.draw_str(&label, (x + pad_x, baseline), &num_font, &num_paint);
}

/// Floating health bar — a narrow rose fill on a wine track, with the live HP
/// numerically beneath. A brief ember "-N.N" damage pop fades out beside the
/// number after each hit. Anchored at the entity's "above-head" screen point.
pub(super) fn draw_floating_health(canvas: &Canvas, ind: &Indicator, fonts: &FontStore) {
    if ind.max_health <= 0.0 {
        return;
    }
    let frac = (ind.health / ind.max_health).clamp(0.0, 1.0);

    // Distance fade — same near/far/min as draw_totem_overhead so the two
    // indicators dim in lockstep on the same entity.
    let alpha_mul = distance_fade_alpha(ind.distance, 6.0, 32.0, 0.35);

    // Bar geometry — fixed width so the indicator stays readable at any
    // distance. Sat above the head; the totem chip stacks higher still.
    let bar_w = 56.0;
    let bar_h = 4.0;
    let cx = ind.screen_x;
    let cy = ind.screen_y;
    let bar_x = cx - bar_w * 0.5;
    let bar_y = cy;

    // Track — wine pill with a hairline inset for legibility on bright maps.
    let track = Rect::from_xywh(bar_x, bar_y, bar_w, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78 * alpha_mul), None);
    canvas.draw_rrect(track_rr, &track_paint);

    // Fill — rose for healthy, ember for low. Threshold at 30% mirrors
    // vanilla's "low HP" heart flash.
    let fill_color = if frac < 0.30 { (0xC9, 0x6A, 0x7A) } else { ROSE };
    let fill_rect = Rect::from_xywh(bar_x, bar_y, bar_w * frac, bar_h);
    if fill_rect.width() > 0.0 {
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(rgba(fill_color, alpha_mul), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    let border_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(0.8);
    border.set_color4f(rgba(ROSE, 0.30 * alpha_mul), None);
    canvas.draw_rrect(border_rr, &border);

    // HP read-out — JetBrains Mono beneath the bar.
    let hp_label = format!("{:.1} / {:.0}", ind.health.max(0.0), ind.max_health);
    let hp_font = fonts.jetbrains_mono(10.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (hp_w, _) = hp_font.measure_str(&hp_label, Some(&probe));
    let (_, m) = hp_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.5 };
    let text_baseline = bar_y + bar_h + cap + 4.0;

    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.65 * alpha_mul), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_str(
        &hp_label,
        (cx - hp_w * 0.5, text_baseline + 1.0),
        &hp_font,
        &shadow,
    );

    let mut hp_paint = Paint::default();
    hp_paint.set_anti_alias(true);
    hp_paint.set_color4f(rgba(PEARL, alpha_mul), None);
    canvas.draw_str(&hp_label, (cx - hp_w * 0.5, text_baseline), &hp_font, &hp_paint);

    // Damage pop — ember "-N.N" beside the HP, fading out over 1.5 s.
    if ind.damage_age_sec >= 0.0 && ind.last_damage > 0.05 {
        let alpha = (1.0 - (ind.damage_age_sec / 1.5).clamp(0.0, 1.0)).powf(1.2) * alpha_mul;
        let dmg_label = format!("-{:.1}", ind.last_damage);
        let dmg_font = fonts.fraunces_axes(13.0, 30.0, 0.0, 600.0, None);
        let mut dmg_paint = Paint::default();
        dmg_paint.set_anti_alias(true);
        let ember = (0xC9, 0x6A, 0x7A);
        dmg_paint.set_color4f(rgba(ember, alpha), None);
        let dmg_x = cx + hp_w * 0.5 + 8.0;
        // Lift the pop as it fades, the standard "damage number" affordance.
        let lift = ind.damage_age_sec * 8.0;
        canvas.draw_str(
            &dmg_label,
            (dmg_x, text_baseline - lift),
            &dmg_font,
            &dmg_paint,
        );
    }
}

/// Hit Indicator chevron — a small ember triangle on a circle around screen
/// centre, pointing outward in the direction the most recent attacker is
/// relative to the player's facing. Fades to zero alpha by `fade_secs`.
///
/// `relative_yaw` is in degrees: 0 = ahead (top of screen), +90 = right,
/// -90 = left, ±180 = behind (bottom). Mapped to a screen-space circle
/// position via `(sin(yaw), -cos(yaw)) × radius`.
pub(super) fn draw_hit_indicator(
    canvas: &Canvas,
    w: f32,
    h: f32,
    relative_yaw_deg: f32,
    age_secs: f32,
    fade_secs: f32,
    radius_pct: f32,
) {
    let progress = (age_secs / fade_secs).clamp(0.0, 1.0);
    let alpha = (1.0 - progress).powf(1.2);
    if alpha <= 0.01 {
        return;
    }

    let cx = w * 0.5;
    let cy = h * 0.5;
    let radius = (w.min(h) * radius_pct * 0.01).min(280.0);

    let yaw_rad = relative_yaw_deg.to_radians();
    let dir_x = yaw_rad.sin();
    let dir_y = -yaw_rad.cos();

    let px = cx + dir_x * radius;
    let py = cy + dir_y * radius;

    // Triangle pointing AWAY from screen centre (toward the attacker bearing).
    let tip_len = 18.0;
    let base_back = 8.0;
    let base_half = 9.0;
    let perp_x = -dir_y;
    let perp_y = dir_x;

    let tip = (px + dir_x * tip_len, py + dir_y * tip_len);
    let base_l = (
        px - dir_x * base_back + perp_x * base_half,
        py - dir_y * base_back + perp_y * base_half,
    );
    let base_r = (
        px - dir_x * base_back - perp_x * base_half,
        py - dir_y * base_back - perp_y * base_half,
    );

    let mut path = skia_safe::Path::new();
    path.move_to(tip);
    path.line_to(base_l);
    path.line_to(base_r);
    path.close();

    let ember = (0xC9, 0x6A, 0x7A);

    // Outer glow — wider stroke under the fill so the chevron pops over busy
    // backgrounds.
    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_style(PaintStyle::Stroke);
    glow.set_stroke_width(6.0);
    glow.set_color4f(rgba(ember, alpha * 0.45), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_path(&path, &glow);

    // Filled chevron — ember body with a rose hairline outline.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(ember, alpha * 0.88), None);
    canvas.draw_path(&path, &fill);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    stroke.set_stroke_width(1.2);
    stroke.set_color4f(rgba(ROSE, alpha), None);
    canvas.draw_path(&path, &stroke);
}

/// Rose "+" painted at screen centre to signal the entity under the crosshair
/// is within attack reach. A two-pass stroke: a soft outer halo first, then a
/// crisp inner stroke. Overlays the vanilla white crosshair (which paints into
/// fbo 0 before the HUD composite), giving a rose halo around the vanilla "+".
pub(super) fn draw_crosshair_on_reach(canvas: &Canvas, w: f32, h: f32) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let arm = 7.0;

    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_style(PaintStyle::Stroke);
    glow.set_stroke_width(6.0);
    glow.set_color4f(rgba(ROSE, 0.55), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_line((cx - arm, cy), (cx + arm, cy), &glow);
    canvas.draw_line((cx, cy - arm), (cx, cy + arm), &glow);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    stroke.set_stroke_width(1.8);
    stroke.set_color4f(rgba(ROSE, 0.95), None);
    canvas.draw_line((cx - arm, cy), (cx + arm, cy), &stroke);
    canvas.draw_line((cx, cy - arm), (cx, cy + arm), &stroke);
}

// ────────────────────────────────────────────────────────────────────────
// HUD editor chrome — drawn over the widgets while the overlay is open.
// ────────────────────────────────────────────────────────────────────────
