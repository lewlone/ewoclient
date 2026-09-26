use super::*;

/// Draw the whole HUD for one frame: place every widget from the persisted
/// layout, record its bounds, and — while the overlay is open — draw the
/// editor chrome on top.
pub fn draw(
    canvas: &Canvas,
    data: &HudData,
    editor: &mut Editor,
    fonts: &FontStore,
    w: f32,
    h: f32,
    frame: Frame,
) {
    // Installed for this call only; cleared by the guard on the way out, even
    // if a draw panics. Every animated widget reads the clock through `now()`
    // and every plate reads the glass sources through `with_glass`.
    let _frame = install_frame(frame);

    editor.window = (w, h);

    // Drain any pending SMTC snapshots into the live media state. The poll is
    // non-blocking — if no update is queued this returns immediately. Split
    // the borrow over disjoint fields so both can be `&mut`.
    if !editor.media_pinned {
        let media_ref = &mut editor.media;
        let service_ref = &mut editor.media_service;
        service_ref.poll(media_ref);
    }
    // One poll per frame, cached — the visualiser is read from the compact
    // widget, the HOME card and the scrub bar, and they must not disagree.
    if !editor.spectrum_pinned {
        editor.spectrum = editor.audio_service.poll();
    }

    // While the overlay is up and the user is on a non-editor tab (HOME /
    // MODULES / MODS / SETTINGS / PVP), hide the in-world widgets so they
    // don't bleed through the overlay panels' translucent chrome. The HUD
    // editor tab still needs them visible — that's where the user is
    // positioning them.
    let show_widgets = !data.overlay_open() || editor.view == OverlayView::HudEditor;
    // Recorded so the input handlers — which run outside `draw`, with no
    // `HudData` — can tell quick-edit apart from the real editor.
    editor.overlay_open = data.overlay_open();

    for id in WidgetId::ALL {
        let wl = editor.layout.get(id);
        let bounds = if show_widgets && wl.enabled && widget_available(id, data) {
            let (ax, ay) = (wl.x * w, wl.y * h);
            let s = wl.scale;

            // Scale about the anchor point, so the anchored corner stays pinned
            // and the widget grows away from it. Widgets draw at their natural
            // size inside the transform and never need to know about it.
            let saved = canvas.save();
            if s != 1.0 {
                canvas.translate((ax, ay));
                canvas.scale((s, s));
                canvas.translate((-ax, -ay));
            }
            // Media is dispatched separately — `draw_widget` only has
            // `&HudData` to work with, while the media state lives on Editor.
            //
            // Cursor is un-scaled into the widget's own space so hover tests
            // inside it (the media transport buttons) still line up.
            let local_cursor = if s != 1.0 {
                (
                    ax + (editor.cursor.0 - ax) / s,
                    ay + (editor.cursor.1 - ay) / s,
                )
            } else {
                editor.cursor
            };
            let local = match id {
                WidgetId::Media => draw_media_compact(
                    canvas,
                    &editor.media,
                    local_cursor,
                    editor.media_button_press,
                    &editor.spectrum,
                    fonts,
                    wl.anchor,
                    ax,
                    ay,
                ),
                _ => draw_widget(canvas, id, data, fonts, wl.anchor, ax, ay),
            };
            canvas.restore_to_count(saved);

            // The widget reported bounds in the scaled space; hit-testing works
            // in screen space.
            if s != 1.0 {
                scale_rect_about(local, ax, ay, s)
            } else {
                local
            }
        } else {
            empty_rect()
        };
        editor.bounds[id.index()] = bounds;
    }

    // Custom crosshair — replaces the vanilla one when enabled. Always
    // drawn at the framebuffer's centre (the Java mixin suppresses
    // vanilla's `Gui.extractCrosshair` when `editor.crosshair.enabled` is
    // true). Three-state colour tracks the entity under the crosshair —
    // idle / target out of reach / target in reach. Skipped on non-editor
    // overlay tabs along with the rest of the in-world HUD.
    if show_widgets && editor.crosshair.enabled {
        let state = editor.crosshair.state_from_target(
            data.target_active(),
            data.target_distance(),
        );
        crate::crosshair::draw(canvas, w * 0.5, h * 0.5, &editor.crosshair, state);
    }

    // Crosshair on Reach module — the legacy fallback that overlays a rose
    // "+" on the vanilla crosshair when an entity under it is within attack
    // reach. Only fires when the custom crosshair is OFF (otherwise the
    // custom crosshair's reach-colour subsumes the same behaviour). Skipped
    // on non-editor overlay tabs.
    if show_widgets && !editor.crosshair.enabled {
        if let Some(idx) = catalog::index_of("crosshair_on_reach") {
            let st = editor.modules.get(idx);
            if st.enabled && data.target_active() && data.target_distance() <= st.settings[0] {
                draw_crosshair_on_reach(canvas, w, h);
            }
        }
    }

    // Hit Indicator module — screen-edge chevron pointing back toward the
    // most recent attacker. Skipped on non-editor overlay tabs (it'd flicker
    // behind the dashboard).
    if show_widgets {
        if let Some(idx) = catalog::index_of("hit_indicator") {
            let st = editor.modules.get(idx);
            if st.enabled && data.hit_present() {
                let radius_pct = st.settings[0].max(5.0).min(50.0);
                let fade_secs = st.settings[1].max(0.1);
                let age = data.hit_age();
                if age >= 0.0 && age < fade_secs {
                    draw_hit_indicator(
                        canvas,
                        w,
                        h,
                        data.hit_relative_yaw(),
                        age,
                        fade_secs,
                        radius_pct,
                    );
                }
            }
        }
    }

    // World-anchored combat indicators — overhead totem-pop counter and
    // floating health/damage on visible LivingEntities. Hidden on non-editor
    // overlay tabs along with the rest of the HUD.
    let totem_on = show_widgets && editor.pvp.totem_count_enabled;
    let health_on = show_widgets && editor.pvp.floating_health_enabled;
    if totem_on || health_on {
        for i in 0..data.indicator_count() {
            let ind = data.indicator(i);
            if !ind.in_view {
                continue;
            }
            if health_on {
                draw_floating_health(canvas, &ind, fonts);
            }
            if totem_on && ind.totem_count > 0 {
                draw_totem_overhead(canvas, &ind, fonts);
            }
        }
    }

    if !data.overlay_open() {
        // Quick-edit: the player is in a vanilla screen with the modifier
        // held. Draw the editor's affordances — outlines, grips, snap guides —
        // over whatever screen they are in, but none of its chrome: no side
        // panel, no tab strip, no scrim. They came here to nudge a widget, not
        // to leave their inventory.
        if editor.quick_edit {
            draw_quick_edit(canvas, editor, fonts, w, h);
        }
        return;
    }

    // Dim the scene — the overlay is a focused mode.
    let mut tint = Paint::default();
    tint.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.22), None);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &tint);

    // Pick up the skin PNGs once the mod has written them — the export
    // finishes after the Editor was constructed, and may replace a stale png
    // from an earlier launch. Polled every SKIN_POLL seconds; reload when the
    // file's (mtime, length) stamp moves. A partial write that failed to
    // decode is retried once the finished write moves the stamp.
    if editor.view == OverlayView::Home && now() - editor.skin_checked_at >= SKIN_POLL {
        editor.skin_checked_at = now();
        let disk_stamp = skin_png_stamp();
        if disk_stamp != editor.skin_stamp {
            editor.skin_stamp = disk_stamp;
            editor.skin_image = load_skin_image("ewo-skin.png");
            editor.cape_image = load_skin_image("ewo-cape.png");
            editor.skin_slim =
                instance_file("ewo-skin-slim").map(|p| p.exists()).unwrap_or(false);
        }
    }

    // The active dashboard view.
    match editor.view {
        OverlayView::Home => draw_home(canvas, editor, data, fonts, w, h),
        OverlayView::HudEditor => draw_editor(canvas, editor, fonts, w, h),
        OverlayView::Crosshair => draw_crosshair_view(canvas, editor, fonts, w, h),
        OverlayView::Modules => draw_modules(canvas, editor, fonts, w, h),
        OverlayView::Pvp => draw_pvp(canvas, editor, fonts, w, h),
        OverlayView::Mods => draw_mods(canvas, editor, fonts, w, h),
        OverlayView::Friends => draw_friends_view(canvas, fonts, w, h),
        OverlayView::Settings => draw_settings(canvas, editor, fonts, w, h),
    }

    // Module settings popover — opened by right-click on a MODULES row that
    // carries sliders. Drawn over the active view but below the tab strip so
    // the user can still see what tab they're on.
    if editor.view == OverlayView::Modules {
        if let Some(idx) = editor.module_popover {
            draw_module_popover(canvas, editor, idx, fonts, w, h);
        }
    }

    // The view-tab strip + a close hint, on top of the view.
    draw_tab_strip(canvas, editor.view, fonts, w);

    let hint_font = fonts.jetbrains_mono(12.0);
    let hint = match editor.view {
        OverlayView::HudEditor => "DRAG WIDGETS OR USE THE PANEL  ·  RIGHT SHIFT OR ESC TO CLOSE",
        OverlayView::Crosshair => "TOGGLE TO REPLACE VANILLA  ·  RIGHT SHIFT OR ESC TO CLOSE",
        OverlayView::Home
        | OverlayView::Modules
        | OverlayView::Pvp
        | OverlayView::Mods
        | OverlayView::Friends
        | OverlayView::Settings => "RIGHT SHIFT OR ESC TO CLOSE",
    };
    let hint_w = measure_tracked_em(&hint_font, hint, 0.14);
    let mut hint_paint = Paint::default();
    hint_paint.set_anti_alias(true);
    hint_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        hint,
        ((w - hint_w) * 0.5, h - 28.0),
        &hint_font,
        &hint_paint,
        0.14,
    );
}

/// Whether `id`'s underlying data is present this frame. PvP widgets show
/// while the overlay editor is open (so they can be placed even without a
/// recent result) or while a real result is live.
pub(super) fn widget_available(id: WidgetId, data: &HudData) -> bool {
    match id {
        WidgetId::Fps => true,
        WidgetId::Ping => data.ping_valid(),
        WidgetId::Coords | WidgetId::Keystrokes => data.world_active(),
        WidgetId::Armor => data.world_active() && data.armor_active(),
        WidgetId::Potions => data.world_active() && data.potion_count() > 0,
        WidgetId::Target => data.world_active() && data.target_active(),
        WidgetId::JumpResetText | WidgetId::JumpResetBar => {
            data.pvp_jump_active() || data.overlay_open()
        }
        WidgetId::HitRange => data.pvp_hit_active() || data.overlay_open(),
        WidgetId::Cps => data.world_active() || data.overlay_open(),
        WidgetId::Items => data.world_active() || data.overlay_open(),
        WidgetId::ShieldCooldown => {
            data.world_active() && (data.shield_cooldown() > 0.0 || data.overlay_open())
        }
        WidgetId::Reach => data.world_active() && (data.target_active() || data.overlay_open()),
        WidgetId::AttackCharge => data.world_active() || data.overlay_open(),
        WidgetId::Combo => data.world_active() && (data.combo_count() > 0 || data.overlay_open()),
        // Media widget is always available — it carries its own empty state
        // when nothing is playing, and the user should be able to position it
        // in the HUD editor regardless of SMTC connection.
        WidgetId::Media => true,
    }
}

/// Draw one widget at `(ax, ay)` with `anchor`; returns its drawn bounds.
pub(super) fn draw_widget(
    canvas: &Canvas,
    id: WidgetId,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    match id {
        WidgetId::Fps => draw_stat(canvas, &data.fps().to_string(), "FPS", fonts, anchor, ax, ay),
        WidgetId::Coords => draw_coords(
            canvas,
            data.player_x(),
            data.player_y(),
            data.player_z(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::Ping => draw_stat(canvas, &data.ping().to_string(), "MS", fonts, anchor, ax, ay),
        WidgetId::Keystrokes => draw_keystrokes(canvas, data.keys(), fonts, anchor, ax, ay),
        WidgetId::Armor => draw_armor(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Potions => draw_potions(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Target => draw_target(canvas, data, fonts, anchor, ax, ay),
        WidgetId::JumpResetText => draw_jump_reset_text(canvas, data, fonts, anchor, ax, ay),
        WidgetId::JumpResetBar => draw_jump_reset_bar(canvas, data, anchor, ax, ay),
        WidgetId::HitRange => draw_hit_range(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Cps => draw_cps(canvas, data.cps_left(), data.cps_right(), fonts, anchor, ax, ay),
        WidgetId::Items => draw_item_counters(
            canvas,
            data.item_pearls(),
            data.item_arrows(),
            data.item_totems(),
            data.item_gapples(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::ShieldCooldown => {
            draw_shield_cooldown(canvas, data.shield_cooldown(), fonts, anchor, ax, ay)
        }
        WidgetId::Reach => draw_stat(
            canvas,
            &format!("{:.2}", data.target_distance().max(0.0)),
            "REACH",
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::AttackCharge => {
            draw_attack_charge(canvas, data.attack_charge(), fonts, anchor, ax, ay)
        }
        WidgetId::Combo => draw_combo(
            canvas,
            data.combo_count(),
            data.combo_age(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        // Media is dispatched separately by the caller — it needs the
        // `MediaState` (carried on `Editor`, not `HudData`), which would
        // bloat this function's signature.
        WidgetId::Media => empty_rect(),
    }
}

/// Compact in-world media controller — the small floating widget. Renders the
/// design's `.w-media` form: 36×36 thumbnail + italic title + tracked artist +
/// three tiny circular transport buttons. Falls back to the empty state when
/// nothing is playing.
pub(super) fn draw_media_compact(
    canvas: &Canvas,
    media: &crate::media::MediaState,
    cursor: (f32, f32),
    press_info: Option<(usize, std::time::Instant)>,
    audio: &crate::audio::Spectrum,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let idle = media.is_idle();
    let has_duration = !idle && media.duration_seconds > 0.0;
    let thumb = 44.0;
    let pad_l = 10.0;
    let pad_y = 10.0;
    let pad_r = 12.0;
    let gap = 10.0;
    let btn = 22.0;
    let btn_gap = 4.0;
    let cluster_w = btn * 3.0 + btn_gap * 2.0;

    // Title font + measurement — the chip is sized to fit the text.
    let title_font = fonts.fraunces_axes(13.0, 100.0, 1.0, 520.0, Some(24.0));
    let artist_font = fonts.jetbrains_mono(9.5);
    let time_font = fonts.jetbrains_mono(9.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);

    let title_str = if idle {
        "Nothing playing".to_string()
    } else {
        media.title.clone()
    };
    let artist_str = if idle {
        "— connect a source —".to_string()
    } else {
        media.artist.clone()
    };
    let (title_w, _) = title_font.measure_str(&title_str, Some(&probe));
    let artist_w = measure_tracked_em(&artist_font, &artist_str, 0.18);
    let text_w = title_w.max(artist_w).min(180.0).max(140.0);

    let chip_w = pad_l + thumb + gap + text_w + gap + cluster_w + pad_r;
    let chip_h = pad_y * 2.0 + thumb;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    // The one plate in the HUD that answers to the music.
    draw_iw_shell_driven(
        canvas,
        chip,
        14.0,
        (!idle).then(|| PlateDrive::from_audio(audio)).flatten(),
    );

    // Thumbnail (44×44, 8px rounded).
    // Spectrum, behind everything else in the text column. Drawn first so the
    // title and scrub bar sit on top of it rather than fighting it.
    // No spectrum bars here, deliberately. The compact widget is 64px tall and
    // every row of it is spoken for — a strip behind the text lands squarely on
    // the timestamps and makes "1:10 / 3:08" unreadable at exactly the moments
    // the music is loud enough to be worth watching. This widget gets its
    // audio reactivity from the scrub bar's liquid surface and the artwork's
    // beat glow, both of which use space that was already there. The bars live
    // on the HOME card, which has the room for them.

    let thumb_rect = Rect::from_xywh(x + pad_l, y + pad_y, thumb, thumb);
    let thumb_rr = RRect::new_rect_xy(thumb_rect, 8.0, 8.0);

    // The beat lands on the artwork: a rose halo that flares on an onset and
    // decays. Cheaper and calmer than scaling the thumbnail, which would
    // resample the image every frame for no extra legibility.
    if audio.pulse > 0.02 && audio.is_live() {
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(ROSE, 0.55 * audio.pulse), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0 + 8.0 * audio.pulse, false));
        canvas.draw_rrect(thumb_rr, &glow);
    }

    if let Some(img) = media.thumbnail.as_ref() {
        canvas.save();
        canvas.clip_rrect(thumb_rr, Some(ClipOp::Intersect), Some(true));
        let mut p = Paint::default();
        p.set_anti_alias(true);
        canvas.draw_image_rect(img, None, thumb_rect, &p);
        canvas.restore();
    } else {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(thumb_rr, &p);
        // Music glyph centred when idle.
        let g_font = fonts.fraunces_axes(22.0, 100.0, 1.0, 500.0, Some(36.0));
        let mut g = Paint::default();
        g.set_anti_alias(true);
        g.set_color4f(rgba(MAUVE, 0.85), None);
        let glyph = "\u{266A}";
        let (gw, _) = g_font.measure_str(glyph, Some(&probe));
        let (_, gm) = g_font.metrics();
        let gcap = if gm.cap_height > 0.0 { gm.cap_height } else { 14.0 };
        canvas.draw_str(
            glyph,
            (thumb_rect.left + (thumb - gw) * 0.5, thumb_rect.top + (thumb + gcap) * 0.5),
            &g_font,
            &g,
        );
    }

    // Text column: title (top), artist (middle), scrub bar + timestamps (bottom)
    // — three rows aligned with the thumbnail height.
    let text_left = thumb_rect.right + gap;
    let title_baseline = y + pad_y + 12.0;
    let artist_baseline = title_baseline + 14.0;
    let scrub_y = thumb_rect.bottom - 6.0;

    canvas.save();
    let text_clip = Rect::new(text_left, y, text_left + text_w, y + chip_h);
    canvas.clip_rect(text_clip, Some(ClipOp::Intersect), Some(true));

    // Marquee scroll — if the title is wider than the visible column, slide
    // it left over time. `MediaState::marquee_offset` does the math; we just
    // translate the title's draw x by `-offset` (the clip rect cuts the
    // overhang on both sides).
    let marquee = media.marquee_offset(title_w, text_w);
    let title_x = text_left - marquee;

    draw_iw_text_shadow(canvas, &title_str, (title_x, title_baseline), &title_font);
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
    canvas.draw_str(&title_str, (title_x, title_baseline), &title_font, &title_paint);

    draw_iw_text_shadow_tracked(canvas, &artist_str, (text_left, artist_baseline), &artist_font, 0.18);
    let mut artist_paint = Paint::default();
    artist_paint.set_anti_alias(true);
    artist_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &artist_str,
        (text_left, artist_baseline),
        &artist_font,
        &artist_paint,
        0.18,
    );
    canvas.restore();

    // ── Scrub row — current "m:ss" + thin track with rose→champ fill +
    //    duration "m:ss". Only when we have a real duration; the empty state
    //    leaves the row blank so it doesn't lie. ─────────────────────────────
    if has_duration {
        let live_pos = media.displayed_position();
        let pos = fmt_track_time(live_pos);
        let dur = fmt_track_time(media.duration_seconds);
        let (pos_w, _) = time_font.measure_str(&pos, Some(&probe));
        let (dur_w, _) = time_font.measure_str(&dur, Some(&probe));

        // Position timestamp on the left of the bar.
        let mut tp = Paint::default();
        tp.set_anti_alias(true);
        tp.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&pos, (text_left, scrub_y + 4.0), &time_font, &tp);

        // Duration timestamp on the right.
        let dur_x = text_left + text_w - dur_w;
        canvas.draw_str(&dur, (dur_x, scrub_y + 4.0), &time_font, &tp);

        // The bar itself — pearl track + a liquid rose→champ fill whose top
        // surface is a travelling wave, plus a pearl knob at the head.
        let bar_left = text_left + pos_w + 6.0;
        let bar_right = dur_x - 6.0;
        // 6px rather than 2px: a wave needs vertical room to read as one. Below
        // ~5px the crest and trough land in the same pixel row and the whole
        // effect collapses into a slightly fuzzy line.
        let bar_h = 6.0;
        let bar_rect = Rect::from_xywh(
            bar_left,
            scrub_y - bar_h * 0.5,
            (bar_right - bar_left).max(0.0),
            bar_h,
        );
        let bar_rr = RRect::new_rect_xy(bar_rect, bar_h * 0.5, bar_h * 0.5);
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(PEARL, 0.12), None);
        canvas.draw_rrect(bar_rr, &bg);

        let frac = (live_pos / media.duration_seconds).clamp(0.0, 1.0);
        draw_liquid_progress(canvas, bar_rect, frac, media.playing, audio);
    }

    // Transport buttons — tiny (22×22) circles, the middle one rose-filled.
    let cluster_left = x + chip_w - pad_r - cluster_w;
    let cy = y + chip_h * 0.5;
    let prev = Rect::from_xywh(cluster_left, cy - btn * 0.5, btn, btn);
    let play = Rect::from_xywh(cluster_left + btn + btn_gap, cy - btn * 0.5, btn, btn);
    let next = Rect::from_xywh(
        cluster_left + (btn + btn_gap) * 2.0,
        cy - btn * 0.5,
        btn,
        btn,
    );

    let playing = media.playing && !idle;
    let (hover_prev, press_prev) = button_state(prev, cursor, 0, press_info);
    let (hover_play, press_play) = button_state(play, cursor, 1, press_info);
    let (hover_next, press_next) = button_state(next, cursor, 2, press_info);
    draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
    draw_media_button(
        canvas,
        play,
        !idle,
        playing,
        if playing { 'p' } else { 'P' },
        hover_play,
        press_play,
    );
    draw_media_button(canvas, next, false, false, '>', hover_next, press_next);

    chip
}

/// Compute `(hovered, press_strength)` for a media transport button.
/// `press_strength` is `1.0` at the moment of click and eases down to `0.0`
/// over `PRESS_FADE_MS`; the button render scales + brightens proportionally.
pub(super) fn button_state(
    rect: Rect,
    cursor: (f32, f32),
    index: usize,
    press: Option<(usize, std::time::Instant)>,
) -> (bool, f32) {
    const PRESS_FADE_MS: f32 = 160.0;
    let hovered = point_in(rect, cursor.0, cursor.1);
    let strength = press
        .and_then(|(i, when)| {
            if i != index {
                return None;
            }
            let elapsed_ms = when.elapsed().as_secs_f32() * 1000.0;
            if elapsed_ms >= PRESS_FADE_MS {
                None
            } else {
                Some(1.0 - elapsed_ms / PRESS_FADE_MS)
            }
        })
        .unwrap_or(0.0);
    (hovered, strength)
}

// ────────────────────────────────────────────────────────────────────────
// Shared helpers.
// ────────────────────────────────────────────────────────────────────────

/// The shared HUD chip — a Velvet re-skin of the prototype's `.hud-stat`
/// background (a translucent rounded panel with a hairline).
///
/// CSS `backdrop-filter: blur(8px)` can't sample the live game — the HUD
/// paints to an offscreen surface (the E1 tradeoff) — so this is a flat wine
/// fill, not a true backdrop blur.
/// Which preset a plate should use. The dashboard's panels are large and calm;
/// in-world widgets are small and want a tighter bevel or the refraction eats
/// the whole surface.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GlassRole {
    Widget,
    Panel,
}

/// Draw `rect` as a refracting glass plate, if this frame can. Returns whether
/// it did — callers fall back to their flat chrome when it returns `false`.
pub(super) fn try_glass_plate(
    canvas: &Canvas,
    rect: Rect,
    radius: f32,
    role: GlassRole,
    drive: Option<PlateDrive>,
) -> bool {
    // Below a certain size the bevel would consume the entire plate and the
    // interior would have no flat region left to carry text.
    if rect.width() < 44.0 || rect.height() < 22.0 {
        return false;
    }
    let time = now();
    with_glass(|src, strength| {
        let base = match role {
            GlassRole::Widget => LiquidGlassParams::WIDGET,
            GlassRole::Panel => LiquidGlassParams::PANEL,
        };
        // Music drive. Each term is chosen for what it does to the *read* of
        // the object, not for magnitude: the ripple makes the surface look
        // agitated, the refraction bulge makes the glass feel like it flexes
        // on a hit, and the rim terms make the edge catch the light. Bounded
        // so a loud passage excites the plate without dissolving it.
        //
        // Weighted toward `pulse` over `level` on the light terms. Sustained
        // loudness driving the rim hard leaves the plate permanently blown out
        // through a loud passage, which stops reading as a reaction at all —
        // it just looks like a brighter widget. Beats punch, loudness glows.
        // `ripple` is the exception: agitation of the *surface* should track
        // how loud it is, and it cannot wash anything out.
        let d = drive.unwrap_or_default();
        let base = LiquidGlassParams {
            ripple: base.ripple * (1.0 + d.level * 2.4),
            specular: base.specular * (1.0 + d.level * 0.55 + d.pulse * 1.1),
            hairline: base.hairline * (1.0 + d.level * 0.30 + d.pulse * 1.3),
            ..base
        };
        let beat_bulge = 1.0 + d.pulse * 0.30;
        // The strength preference scales the physical knobs, not the tint —
        // turning the glass down should flatten the bevel, not repaint the
        // surface a different colour.
        let short_side = rect.width().min(rect.height());
        let params = LiquidGlassParams {
            radius,
            // Never let the bevel exceed a third of the short side, or the
            // "flat centre" guarantee that keeps text readable is lost.
            edge: (base.edge * strength).min(short_side / 3.0),
            strength: base.strength * strength * beat_bulge,
            disperse: base.disperse * strength * beat_bulge,
            ..base
        };
        draw_liquid_glass(
            canvas,
            rect,
            GlassBackdrop {
                rim: &src.rim,
                rim_scale: src.rim_scale,
                frost: &src.frost,
                frost_scale: src.frost_scale,
            },
            params,
            time,
        )
    })
    .unwrap_or(false)
}

pub(super) fn draw_chip(canvas: &Canvas, rect: Rect, radius: f32) {
    if try_glass_plate(canvas, rect, radius, GlassRole::Panel, None) {
        return;
    }

    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.62), None);
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.12), None);
    canvas.draw_rrect(rrect, &border);
}

/// How hard the music is driving a plate this frame.
///
/// The music does not get its own chart — the *widget* responds. A spectrum
/// strip was built first and removed: over a 64px widget it lands on the text,
/// and even on the roomier HOME card it sat behind the artist line and the
/// timestamps. More to the point it was the wrong idea, a graph pasted onto an
/// object instead of the object behaving.
///
/// Everything driven here lives in the *plate* — rim light, edge hairline,
/// refraction, bloom. Nothing touches the content layer, which is what keeps
/// this on the right side of the "never animate anything containing text"
/// rule: the glass flexes, the words hold perfectly still.
#[derive(Clone, Copy, Default)]
pub(super) struct PlateDrive {
    /// Sustained loudness, 0..1 — how excited the glass is overall.
    pub(super) level: f32,
    /// Onset spike, 0..1 — the hit.
    pub(super) pulse: f32,
}

impl PlateDrive {
    /// Drive from a spectrum, or nothing when there is no audio to react to.
    pub(super) fn from_audio(audio: &crate::audio::Spectrum) -> Option<PlateDrive> {
        audio
            .is_live()
            .then(|| PlateDrive { level: audio.level.clamp(0.0, 1.0), pulse: audio.pulse.clamp(0.0, 1.0) })
    }
}

/// Cached target-skin PNG, keyed by the name the mod published it for.
///
/// `Image` is a refcounted handle, so re-cloning it per frame is free; the
/// point of the cache is to avoid re-decoding the PNG, and to avoid stat-ing
/// the marker file more than a few times a second.
pub(super) struct TargetSkinCache {
    /// Name from `ewo-target.txt` at the last read.
    pub(super) name: String,
    pub(super) image: Option<Image>,
    /// `ewo-target.png`'s mtime when it was decoded.
    pub(super) mtime: Option<std::time::SystemTime>,
    /// Frame time of the last disk check.
    pub(super) checked_at: f32,
}

thread_local! {
    static TARGET_SKIN: RefCell<TargetSkinCache> = const { RefCell::new(TargetSkinCache {
        name: String::new(),
        image: None,
        mtime: None,
        checked_at: f32::NEG_INFINITY,
    }) };
}

/// How often to re-read the target marker + PNG. The crosshair can cross a
/// dozen players a second; the disk should not.
pub(super) const TARGET_SKIN_POLL: f32 = 0.25;

/// Draw the targeted player's face into `avatar`, if the mod has exported a
/// skin for exactly this `name`. Returns whether it did.
///
/// The name check is the whole safety property: `EwoTargetSkin` writes the PNG
/// first and the name marker second, so a marker that matches means the PNG
/// beside it is settled and belongs to this player. Any mismatch — a mob, a
/// player whose skin has not arrived, a swap caught mid-write — falls back to
/// the monogram rather than showing someone else's face.
pub(super) fn draw_player_head(canvas: &Canvas, rr: RRect, avatar: Rect, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let image = TARGET_SKIN.with(|cell| {
        let mut c = cell.borrow_mut();
        let t = now();
        if t - c.checked_at >= TARGET_SKIN_POLL {
            c.checked_at = t;
            let published = instance_file("ewo-target.txt")
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let mtime = instance_file("ewo-target.png")
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());
            if published != c.name || mtime != c.mtime {
                c.name = published;
                c.mtime = mtime;
                c.image = load_skin_image("ewo-target.png");
            }
        }
        if c.name == name {
            c.image.clone()
        } else {
            None
        }
    });
    let Some(skin) = image else {
        return false;
    };

    // A Minecraft skin is 64×64; the face is the 8×8 at (8,8) and the hat
    // overlay the 8×8 at (40,8). Both are drawn with a nearest-neighbour
    // sampling so the pixel art stays pixel art at 44px — a smooth upscale
    // would turn a face into a smudge.
    let saved = canvas.save();
    canvas.clip_rrect(rr, Some(ClipOp::Intersect), Some(true));

    let mut paint = Paint::default();
    paint.set_anti_alias(false);
    let sampling = skia_safe::SamplingOptions::from(skia_safe::FilterMode::Nearest);

    let face = Rect::from_xywh(8.0, 8.0, 8.0, 8.0);
    canvas.draw_image_rect_with_sampling_options(&skin, Some((&face, skia_safe::canvas::SrcRectConstraint::Strict)), avatar, sampling, &paint);

    // Hat layer, if the skin has one. Drawn at the same size — vanilla insets
    // the hat slightly in 3D, but flat-on the two layers are coincident.
    let hat = Rect::from_xywh(40.0, 8.0, 8.0, 8.0);
    canvas.draw_image_rect_with_sampling_options(&skin, Some((&hat, skia_safe::canvas::SrcRectConstraint::Strict)), avatar, sampling, &paint);

    canvas.restore_to_count(saved);

    // Keep the rose hairline so the tile still reads as part of the widget.
    let mut edge = Paint::default();
    edge.set_anti_alias(true);
    edge.set_style(PaintStyle::Stroke);
    edge.set_stroke_width(1.0);
    edge.set_color4f(rgba(ROSE, 0.55), None);
    canvas.draw_rrect(rr, &edge);
    true
}

/// The media scrub bar's fill: liquid in a tube, with a travelling wave for a
/// surface.
///
/// The wave runs only while the track is *playing* — a paused bar goes still.
/// That is the whole reason to animate it: motion that never stops is
/// decoration, motion tied to state is information you can read at a glance.
///
/// Amplitude ramps toward the leading edge so the head of the liquid ripples
/// and the settled tail behind it is calm.
pub(super) fn draw_liquid_progress(
    canvas: &Canvas,
    bar: Rect,
    frac: f32,
    playing: bool,
    audio: &crate::audio::Spectrum,
) {
    let fill_w = bar.width() * frac.clamp(0.0, 1.0);
    if fill_w <= 1.0 {
        return;
    }

    // Clip to the track so the wave can never spill out of the rounded tube.
    let saved = canvas.save();
    canvas.clip_rrect(
        RRect::new_rect_xy(bar, bar.height() * 0.5, bar.height() * 0.5),
        Some(ClipOp::Intersect),
        Some(true),
    );

    const WAVELENGTH: f32 = 24.0;
    const SPEED: f32 = 2.4; // radians/sec

    // Water in a speaker cabinet: the surface is driven by what is actually
    // coming out. With live audio, amplitude tracks loudness and a beat kicks
    // it further; with none — muted, or no capture backend — it falls back to
    // the fixed idle ripple so the bar still looks alive rather than dead.
    let (amp, speed) = if !playing {
        (0.0, SPEED)
    } else if audio.is_live() {
        let drive = audio.level.clamp(0.0, 1.0);
        (0.5 + drive * 2.2 + audio.pulse * 0.9, SPEED * (0.7 + drive * 1.1))
    } else {
        (1.3, SPEED)
    };
    let phase = now() * speed;
    // Rest a touch below centre so the tube still reads as part-full even at
    // zero amplitude.
    let mid = bar.top + bar.height() * 0.42;

    let mut path = skia_safe::Path::new();
    path.move_to((bar.left, bar.bottom));
    // ~2px per segment: fine enough that the crest is smooth, coarse enough
    // that a full-width bar is still only a couple of hundred points.
    let steps = (fill_w / 2.0).ceil().max(2.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let px = bar.left + fill_w * t;
        // Taper: calm at the tail, liveliest at the head.
        let local_amp = amp * (0.35 + 0.65 * t);
        let py = mid
            + local_amp * (px / WAVELENGTH * std::f32::consts::TAU - phase).sin()
            // A second, slower harmonic keeps it from looking like a test
            // signal — real liquid never repeats on one period.
            + local_amp * 0.4 * (px / (WAVELENGTH * 2.3) * std::f32::consts::TAU + phase * 0.6).sin();
        path.line_to((px, py));
    }
    path.line_to((bar.left + fill_w, bar.bottom));
    path.close();

    let mut fp = Paint::default();
    fp.set_anti_alias(true);
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(bar.left, bar.top),
            Point::new(bar.right, bar.top),
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
    canvas.draw_path(&path, &fp);

    canvas.restore_to_count(saved);

    // Pearl knob at the head, breathing gently while playing.
    let knob_x = bar.left + fill_w;
    let knob_y = bar.center_y();
    let pulse = if playing {
        1.0 + 0.12 * (now() * 3.1).sin()
    } else {
        1.0
    };
    let mut halo = Paint::default();
    halo.set_anti_alias(true);
    halo.set_color4f(rgba(PEARL, 0.35), None);
    halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_circle((knob_x, knob_y), 5.0 * pulse, &halo);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_circle((knob_x, knob_y), 3.0 * pulse, &knob);
}

/// In-world widget shell — the canonical glass plate every floating widget sits
/// on. Calibrated to read on ANY backdrop (bright snow + pitch caves + lava) via
/// a layered chrome stack:
///
///   1. Drop shadow                  — lifts the widget off the world
///   2. Outer dark wine ring         — separates from bright backgrounds
///   3. Translucent wine fill        — lets the game show through
///   4. Inset wine ring              — soft inner edge
///   5. Inset top pearl highlight    — top-edge gloss (1px strip)
///   6. Pearl border                 — crisp outer edge
///
/// This is the design's `.iw-shell` recipe (Velvet & Pearl redesign), with the
/// fill bumped from CSS `0.32` to `0.50` to compensate for the lack of
/// `backdrop-filter: blur(8px)` — we paint to an offscreen surface, so the
/// game pixels aren't available to blur until composite time.
pub(super) fn draw_iw_shell(canvas: &Canvas, rect: Rect, radius: f32) {
    draw_iw_shell_driven(canvas, rect, radius, None);
}

/// The widget plate, optionally driven by the music.
///
/// `drive` is `Some` only for the media widget — nothing else should twitch
/// when a bass note lands.
pub(super) fn draw_iw_shell_driven(canvas: &Canvas, rect: Rect, radius: f32, drive: Option<PlateDrive>) {
    // An outer bloom that swells with the music. Drawn under everything, so it
    // reads as the plate *glowing* rather than as a ring stuck to it. This is
    // the part that carries the reaction when the plate is small — a 64px
    // widget has very little rim to light up, but it has as much halo as it
    // wants.
    if let Some(d) = drive {
        let energy = d.level * 0.30 + d.pulse * 0.70;
        if energy > 0.01 {
            let mut bloom = Paint::default();
            bloom.set_anti_alias(true);
            bloom.set_color4f(rgba(ROSE, 0.42 * energy), None);
            bloom.set_mask_filter(MaskFilter::blur(
                BlurStyle::Normal,
                10.0 + 16.0 * energy,
                false,
            ));
            // Grow the halo outward with the beat. Inflating a *shadow* rect is
            // free of the text-rasterisation problem that scaling the plate
            // itself would cause — nothing inside is being transformed.
            let grow = 2.0 + 6.0 * d.pulse;
            let halo = Rect::new(
                rect.left - grow,
                rect.top - grow,
                rect.right + grow,
                rect.bottom + grow,
            );
            canvas.draw_rrect(RRect::new_rect_xy(halo, radius + grow, radius + grow), &bloom);
        }
    }

    // Refracting glass when this frame carries a game snapshot; the flat plate
    // below otherwise. The fallback is load-bearing, not defensive politeness —
    // the snapshot is absent for the first frame after launch or a resize, and
    // a HUD that vanished on those frames would be worse than a flat one.
    if try_glass_plate(canvas, rect, radius, GlassRole::Widget, drive) {
        return;
    }

    // (1) Drop shadow.
    let shadow_rect = rect.with_offset((0.0, 6.0));
    let shadow_rr = RRect::new_rect_xy(shadow_rect, radius, radius);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0, false));
    canvas.draw_rrect(shadow_rr, &shadow);

    // (2) Outer dark wine ring — 1px stroke 1px outside the shell.
    let outer = Rect::new(rect.left - 1.0, rect.top - 1.0, rect.right + 1.0, rect.bottom + 1.0);
    let outer_rr = RRect::new_rect_xy(outer, radius + 1.0, radius + 1.0);
    let mut outer_paint = Paint::default();
    outer_paint.set_anti_alias(true);
    outer_paint.set_style(PaintStyle::Stroke);
    outer_paint.set_stroke_width(1.0);
    outer_paint.set_color4f(rgba(WINE, 0.55), None);
    canvas.draw_rrect(outer_rr, &outer_paint);

    let rrect = RRect::new_rect_xy(rect, radius, radius);

    // (3) Fill.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.50), None);
    canvas.draw_rrect(rrect, &fill);

    // (4) Inset wine ring — 1px stroke 1px inside the shell.
    let inset_wine = Rect::new(rect.left + 1.0, rect.top + 1.0, rect.right - 1.0, rect.bottom - 1.0);
    let inset_r = (radius - 1.0).max(0.0);
    let mut wine_paint = Paint::default();
    wine_paint.set_anti_alias(true);
    wine_paint.set_style(PaintStyle::Stroke);
    wine_paint.set_stroke_width(1.0);
    wine_paint.set_color4f(rgba(WINE, 0.25), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset_wine, inset_r, inset_r), &wine_paint);

    // (5) Inset top pearl highlight — clip to the top 2px and stroke the rrect.
    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset_top = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let top_r = (radius - 0.5).max(0.0);
    let mut top_paint = Paint::default();
    top_paint.set_anti_alias(true);
    top_paint.set_style(PaintStyle::Stroke);
    top_paint.set_stroke_width(1.0);
    top_paint.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset_top, top_r, top_r), &top_paint);
    canvas.restore();

    // (6) Pearl border — outermost edge. Brightens with the music when driven:
    // the flat plate has no bevel to catch light, so the border is where its
    // reaction has to live. Without this, turning glass off would also turn
    // the music off, which is not what that setting means.
    let d = drive.unwrap_or_default();
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0 + d.pulse * 0.8);
    border.set_color4f(rgba(PEARL, 0.18 + d.level * 0.32 + d.pulse * 0.28), None);
    canvas.draw_rrect(rrect, &border);
}

/// Three-stop dark velvet text shadow — the in-world legibility recipe. Draw
/// this *before* the actual text. Three blurred wine layers stack into a halo
/// that holds glyphs against snow, lava, and dark caves alike.
///
/// Caller draws the final text in their chosen color on top.
pub(super) fn draw_iw_text_shadow(canvas: &Canvas, text: &str, pos: (f32, f32), font: &Font) {
    let (x, y) = pos;

    // Wider halo — sigma 5 (CSS blur 10px).
    let mut wide = Paint::default();
    wide.set_anti_alias(true);
    wide.set_color4f(rgba(WINE, 0.55), None);
    wide.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
    canvas.draw_str(text, (x, y), font, &wide);

    // Tight halo — sigma 3 (CSS blur 6px).
    let mut tight = Paint::default();
    tight.set_anti_alias(true);
    tight.set_color4f(rgba(WINE, 0.85), None);
    tight.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_str(text, (x, y), font, &tight);

    // Hard underline — no blur, 1px down.
    let mut hard = Paint::default();
    hard.set_anti_alias(true);
    hard.set_color4f(rgba(WINE, 0.95), None);
    canvas.draw_str(text, (x, y + 1.0), font, &hard);
}

/// Tracked-em variant of `draw_iw_text_shadow` — for letter-spaced labels
/// (eyebrows, units) drawn with `draw_tracked_em`.
pub(super) fn draw_iw_text_shadow_tracked(
    canvas: &Canvas,
    text: &str,
    pos: (f32, f32),
    font: &Font,
    tracking_em: f32,
) {
    let (x, y) = pos;

    let mut wide = Paint::default();
    wide.set_anti_alias(true);
    wide.set_color4f(rgba(WINE, 0.55), None);
    wide.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
    draw_tracked_em(canvas, text, (x, y), font, &wide, tracking_em);

    let mut tight = Paint::default();
    tight.set_anti_alias(true);
    tight.set_color4f(rgba(WINE, 0.85), None);
    tight.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    draw_tracked_em(canvas, text, (x, y), font, &tight, tracking_em);

    let mut hard = Paint::default();
    hard.set_anti_alias(true);
    hard.set_color4f(rgba(WINE, 0.95), None);
    draw_tracked_em(canvas, text, (x, y + 1.0), font, &hard, tracking_em);
}
