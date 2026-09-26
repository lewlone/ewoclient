use super::*;

/// One bundled mod in the MODS view, decoded from `overlay-mods.toml` (which
/// the launcher writes before each launch).
pub(super) struct ModEntry {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) category: String,
    pub(super) version: String,
    pub(super) enabled: bool,
    /// Enabled state when loaded — the override file carries only the mods
    /// whose `enabled` now differs from this.
    pub(super) original: bool,
}

/// `<instance-dir>/<file>` — the cdylib runs with the instance dir as its CWD.
pub(super) fn instance_file(file: &str) -> Option<PathBuf> {
    std::env::current_dir().ok().map(|d| d.join(file))
}

/// Load a skin / cape PNG the mod wrote into the instance dir, if present.
pub(super) fn load_skin_image(name: &str) -> Option<Image> {
    let bytes = std::fs::read(instance_file(name)?).ok()?;
    Image::from_encoded(Data::new_copy(&bytes))
}

/// Seconds between `ewo-skin.png` stats while the HOME view is open.
pub(super) const SKIN_POLL: f32 = 1.0;

/// `ewo-skin.png`'s (last-modified, length), or `None` if it isn't there yet.
pub(super) fn skin_png_stamp() -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(instance_file("ewo-skin.png")?).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Read the launcher-written `ewo-keybinds.txt` from the instance dir into
/// (action_id, glfw_code) pairs. Mirrors `EwoKeybinds.java`'s parser — each
/// line is `action=code` or `action=code:mods`, blank/`#`-prefixed lines skip.
/// Returns an empty vec if the file is absent — every action is then unbound,
/// which is fine (the MODULES tab just hides the chip).
pub(super) fn load_keybinds() -> Vec<(String, i32)> {
    let Some(path) = instance_file("ewo-keybinds.txt") else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, i32)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((id, value)) = line.split_once('=') else {
            continue;
        };
        let id = id.trim();
        let value = value.trim();
        let value = value.split_once(':').map(|(c, _)| c).unwrap_or(value).trim();
        if let Ok(code) = value.parse::<i32>() {
            out.push((id.to_string(), code));
        }
    }
    out
}

/// Short uppercase label for a GLFW key code — covers the keys the keybind
/// registry's defaults reach for (letters, digits, function keys, modifiers,
/// arrows, common navigation). Anything else falls back to `KEY <code>`.
pub(super) fn glfw_key_label(code: i32) -> String {
    match code {
        32 => "SPACE".to_string(),
        39 => "'".to_string(),
        44 => ",".to_string(),
        45 => "-".to_string(),
        46 => ".".to_string(),
        47 => "/".to_string(),
        48..=57 => ((b'0' + (code - 48) as u8) as char).to_string(),
        59 => ";".to_string(),
        61 => "=".to_string(),
        65..=90 => ((b'A' + (code - 65) as u8) as char).to_string(),
        91 => "[".to_string(),
        92 => "\\".to_string(),
        93 => "]".to_string(),
        96 => "`".to_string(),
        256 => "ESC".to_string(),
        257 => "ENTER".to_string(),
        258 => "TAB".to_string(),
        259 => "BKSP".to_string(),
        260 => "INS".to_string(),
        261 => "DEL".to_string(),
        262 => "→".to_string(),
        263 => "←".to_string(),
        264 => "↓".to_string(),
        265 => "↑".to_string(),
        266 => "PGUP".to_string(),
        267 => "PGDN".to_string(),
        268 => "HOME".to_string(),
        269 => "END".to_string(),
        280 => "CAPS".to_string(),
        281 => "SCRL".to_string(),
        282 => "NUM".to_string(),
        290..=301 => format!("F{}", code - 289),
        320..=329 => format!("NUM{}", code - 320),
        336 => "ENTER".to_string(),
        340 => "LSHFT".to_string(),
        341 => "LCTRL".to_string(),
        342 => "LALT".to_string(),
        343 => "LSUPER".to_string(),
        344 => "RSHFT".to_string(),
        345 => "RCTRL".to_string(),
        346 => "RALT".to_string(),
        347 => "RSUPER".to_string(),
        _ => format!("KEY {}", code),
    }
}

/// Read `overlay-mods.toml` (written by the launcher) into the MODS list.
pub(super) fn load_mods() -> Vec<ModEntry> {
    let Some(path) = instance_file("overlay-mods.toml") else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut mods: Vec<ModEntry> = Vec::new();
    let mut cur: Option<ModEntry> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(m) = cur.take() {
                mods.push(m);
            }
            cur = Some(ModEntry {
                id: id.to_string(),
                name: id.to_string(),
                category: String::new(),
                version: String::new(),
                enabled: true,
                original: true,
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if let Some(m) = cur.as_mut() {
            match key.trim() {
                "name" => m.name = value.to_string(),
                "category" => m.category = value.to_string(),
                "version" => m.version = value.to_string(),
                "enabled" => {
                    m.enabled = value == "true";
                    m.original = m.enabled;
                }
                _ => {}
            }
        }
    }
    if let Some(m) = cur.take() {
        mods.push(m);
    }
    mods
}

/// Write `overlay-mod-overrides.toml` — only the mods toggled away from their
/// loaded state. The launcher consumes (and deletes) it on the next launch.
pub(super) fn save_mod_overrides(mods: &[ModEntry]) {
    let Some(path) = instance_file("overlay-mod-overrides.toml") else {
        return;
    };
    let mut s = String::from("# In-game mod toggles — consumed by the launcher next launch.\n");
    for m in mods {
        if m.enabled != m.original {
            s.push_str(&format!("{} = {}\n", m.id, m.enabled));
        }
    }
    let _ = std::fs::write(&path, s);
}

/// Velvet accent for a bundled-mod category — the row's colour dot.
pub(super) fn category_color(category: &str) -> (u8, u8, u8) {
    match category {
        "performance" => ROSE,
        "visuals" => LAV,
        "utility" => CHAMP,
        "social" => BERRY,
        _ => MAUVE, // library / unknown
    }
}

/// The Mods-view panel rect + a toggle rect per mod row.
pub(super) fn mods_layout(w: f32, h: f32, count: usize) -> (Rect, Vec<Rect>) {
    const PANEL_W: f32 = 544.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 76.0;
    const ROW_H: f32 = 34.0;
    let panel_h = PAD * 2.0 + HEADER_H + count.max(1) as f32 * ROW_H;
    let px = (w - PANEL_W) * 0.5;
    let py = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let rows_top = py + PAD + HEADER_H;
    const TOGGLE_W: f32 = 38.0;
    const TOGGLE_H: f32 = 20.0;
    let mut toggles = Vec::with_capacity(count);
    for i in 0..count {
        let mid = rows_top + i as f32 * ROW_H + ROW_H * 0.5;
        toggles.push(Rect::from_xywh(
            px + PANEL_W - PAD - TOGGLE_W,
            mid - TOGGLE_H * 0.5,
            TOGGLE_W,
            TOGGLE_H,
        ));
    }
    (panel, toggles)
}

/// The FRIENDS view — who's on the chickenedin network right now, read from
/// the `ewo-friends.txt` snapshot the launcher drops in the active profile
/// dir (Phase H). Read-only: a Velvet list with an online dot · name ·
/// presence per row. Mutations + join stay on the launcher for now.
pub(super) fn draw_friends_view(canvas: &Canvas, fonts: &FontStore, w: f32, h: f32) {
    let friends = crate::social::read_friends();
    let online_n = friends.iter().filter(|f| f.online).count();

    const PANEL_W: f32 = 544.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 76.0;
    const ROW_H: f32 = 40.0;
    let rows = friends.len().max(1);
    let panel_h = PAD * 2.0 + HEADER_H + rows as f32 * ROW_H;
    let px = (w - PANEL_W) * 0.5;
    let py = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 24.0;

    // Eyebrow + online count.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(canvas, "FRIENDS", (left, panel.top + 36.0), &eyebrow_font, &eyebrow, 0.22);

    let count_str = format!("{} / {} ONLINE", online_n, friends.len());
    let count_w = measure_tracked_em(&eyebrow_font, &count_str, 0.16);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &count_str,
        (panel.right - 24.0 - count_w, panel.top + 36.0),
        &eyebrow_font,
        &count_paint,
        0.16,
    );

    // Title.
    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("Who's around", (left, panel.top + 70.0), &title_font, &title);

    if friends.is_empty() {
        let body_font = fonts.newsreader(15.0);
        let mut body = Paint::default();
        body.set_anti_alias(true);
        body.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(
            "No friends yet — link the launcher and add friends from the",
            (left, panel.top + 112.0),
            &body_font,
            &body,
        );
        canvas.draw_str(
            "launcher's Friends screen. They'll show here once they're online.",
            (left, panel.top + 134.0),
            &body_font,
            &body,
        );
        return;
    }

    let rows_top = panel.top + PAD + HEADER_H;
    let name_font = fonts.newsreader(15.0);
    let meta_font = fonts.jetbrains_mono(10.0);
    let (_, nm) = name_font.metrics();
    let ncap = if nm.cap_height > 0.0 { nm.cap_height } else { 11.0 };
    for (i, f) in friends.iter().enumerate() {
        let ry = rows_top + i as f32 * ROW_H;
        let mid = ry + ROW_H * 0.5;

        if i > 0 {
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_style(PaintStyle::Stroke);
            div.set_stroke_width(1.0);
            div.set_color4f(rgba(PEARL, 0.06), None);
            canvas.draw_line((left, ry), (panel.right - 24.0, ry), &div);
        }

        // Online dot — lavender (with halo) when online, dim mauve otherwise.
        let dot_cx = left + 6.0;
        if f.online {
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(LAV, 0.55), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
            canvas.draw_circle((dot_cx, mid), 5.5, &halo);
        }
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            if f.online { rgba(LAV, 1.0) } else { rgba(MAUVE, 0.4) },
            None,
        );
        canvas.draw_circle((dot_cx, mid), 4.0, &dot);

        // Name.
        let mut name = Paint::default();
        name.set_anti_alias(true);
        name.set_color4f(
            if f.online { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.7) },
            None,
        );
        canvas.draw_str(&f.name, (left + 20.0, mid + ncap * 0.5), &name_font, &name);

        // Presence, right-aligned, tracked mono. Champagne when in-game,
        // rose when in the launcher, dim mauve when offline.
        let in_game = !f.server_addr.is_empty();
        let pres = if f.presence.is_empty() {
            "offline".to_string()
        } else {
            f.presence.clone()
        };
        let pres_up = pres.to_uppercase();
        let pres_w = measure_tracked_em(&meta_font, &pres_up, 0.14);
        let mut pres_paint = Paint::default();
        pres_paint.set_anti_alias(true);
        pres_paint.set_color4f(
            if in_game {
                rgba(CHAMP, 0.95)
            } else if f.online {
                rgba(ROSE, 0.85)
            } else {
                rgba(MAUVE, 0.7)
            },
            None,
        );
        draw_tracked_em(
            canvas,
            &pres_up,
            (panel.right - 24.0 - pres_w, mid + 3.0),
            &meta_font,
            &pres_paint,
            0.14,
        );
    }
}

/// The Mods view — a Velvet re-skin of a ClickGUI module list: one row per
/// bundled mod (category dot · name · category·version · on/off toggle).
pub(super) fn draw_mods(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let mods = &editor.mods;
    let (panel, toggles) = mods_layout(w, h, mods.len());
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 24.0;

    // Eyebrow + the enabled count.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(canvas, "MODS", (left, panel.top + 36.0), &eyebrow_font, &eyebrow, 0.22);

    let on_count = mods.iter().filter(|m| m.enabled).count();
    let count_str = format!("{} / {} ENABLED", on_count, mods.len());
    let count_w = measure_tracked_em(&eyebrow_font, &count_str, 0.16);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &count_str,
        (panel.right - 24.0 - count_w, panel.top + 36.0),
        &eyebrow_font,
        &count_paint,
        0.16,
    );

    // Title.
    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("Bundled mods", (left, panel.top + 70.0), &title_font, &title);

    if mods.is_empty() {
        let body_font = fonts.newsreader(15.0);
        let mut body = Paint::default();
        body.set_anti_alias(true);
        body.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(
            "No bundled mods found — launch an Ewo instance to populate this.",
            (left, panel.top + 110.0),
            &body_font,
            &body,
        );
        return;
    }

    // Rows.
    let rows_top = panel.top + 24.0 + 76.0;
    let row_h = 34.0;
    let name_font = fonts.newsreader(15.0);
    let meta_font = fonts.jetbrains_mono(11.0);
    let (_, nm) = name_font.metrics();
    let ncap = if nm.cap_height > 0.0 { nm.cap_height } else { 11.0 };
    for (i, m) in mods.iter().enumerate() {
        let ry = rows_top + i as f32 * row_h;
        let mid = ry + row_h * 0.5;

        // Hairline divider above every row but the first.
        if i > 0 {
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_style(PaintStyle::Stroke);
            div.set_stroke_width(1.0);
            div.set_color4f(rgba(PEARL, 0.06), None);
            canvas.draw_line((left, ry), (panel.right - 24.0, ry), &div);
        }

        // Off-state fade — apply to the whole row at once (design's
        // `.mod-row.off { opacity: .55 }`).
        let row_alpha = if m.enabled { 1.0 } else { 0.55 };

        // Category LED — 4px disc with a soft category-tinted halo when on
        // (design's `box-shadow: 0 0 8px <color>`), dark wine when off.
        let led_cx = left + 7.0;
        let led_color = category_color(&m.category);
        if m.enabled {
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(led_color, 0.6), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
            canvas.draw_circle((led_cx, mid), 5.5, &halo);
        }
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            if m.enabled {
                rgba(led_color, row_alpha)
            } else {
                Color4f::new(
                    0x3A as f32 / 255.0,
                    0x1A as f32 / 255.0,
                    0x23 as f32 / 255.0,
                    1.0,
                )
            },
            None,
        );
        canvas.draw_circle((led_cx, mid), 4.0, &dot);

        // Name.
        let mut name = Paint::default();
        name.set_anti_alias(true);
        name.set_color4f(
            if m.enabled {
                rgba(PEARL, 1.0)
            } else {
                rgba(MAUVE, 0.7)
            },
            None,
        );
        canvas.draw_str(&m.name, (left + 22.0, mid + ncap * 0.5), &name_font, &name);

        // Category · version, tracked, before the toggle.
        let meta = format!("{}  ·  {}", m.category.to_uppercase(), m.version);
        let meta_w = measure_tracked_em(&meta_font, &meta, 0.18);
        let mut meta_paint = Paint::default();
        meta_paint.set_anti_alias(true);
        meta_paint.set_color4f(rgba(MAUVE, 0.85 * row_alpha), None);
        draw_tracked_em(
            canvas,
            &meta,
            (toggles[i].left - 16.0 - meta_w, mid + 4.0),
            &meta_font,
            &meta_paint,
            0.18,
        );

        draw_panel_toggle(canvas, toggles[i], m.enabled);
    }
}

// ────────────────────────────────────────────────────────────────────────
// PVP view — the PvP-Utils editor: master toggles, per-tier sounds, zones.
// (Sprint 2a) Edits are saved to the active profile's `pvp.toml`; the Java
// mod polls the file's mtime each frame and hot-reloads, so changes apply
// live without a relaunch.
// ────────────────────────────────────────────────────────────────────────
