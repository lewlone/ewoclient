use super::*;

impl Editor {
    /// Build the editor, loading the persisted layout from `hud.toml`.
    pub fn new() -> Self {
        let (profile_active, profile_list) = read_profiles()
            .unwrap_or_else(|| ("Default".to_string(), vec!["Default".to_string()]));
        let mut e = Editor {
            view: OverlayView::Home,
            layout: HudLayout::load(),
            window: (1.0, 1.0),
            cursor: (0.0, 0.0),
            bounds: [empty_rect(); WIDGET_COUNT],
            dragging: None,
            resizing: None,
            media_pinned: false,
            spectrum_pinned: false,
            quick_edit: false,
            overlay_open: false,
            slider_drag: None,
            snap_x: None,
            snap_y: None,
            selected: None,
            mods: load_mods(),
            modules: crate::modules::ModuleConfig::load(),
            active_profile: profile_active,
            profiles: profile_list,
            skin_image: load_skin_image("ewo-skin.png"),
            cape_image: load_skin_image("ewo-cape.png"),
            skin_yaw: 0.5,
            skin_drag: None,
            skin_slim: instance_file("ewo-skin-slim").map(|p| p.exists()).unwrap_or(false),
            skin_stamp: skin_png_stamp(),
            skin_checked_at: f32::NEG_INFINITY,
            pvp: crate::pvp::PvpConfig::load(),
            pvp_drag: None,
            glass_drag: false,
            keybinds: load_keybinds(),
            media: crate::media::MediaState::empty(),
            media_service: crate::media::MediaService::start(),
            audio_service: crate::audio::AudioService::start(),
            spectrum: crate::audio::Spectrum::SILENT,
            modules_scroll: 0.0,
            home_toggle_bounds: [empty_rect(); WIDGET_COUNT],
            module_popover: None,
            media_button_press: None,
            crosshair: crate::crosshair::load(),
            crosshair_ui: CrosshairUi::default(),
        };
        // Drop the picker into sync with the loaded crosshair's idle colour
        // so the SV-square cursor / hue strip / alpha strip all start where
        // the loaded swatch actually sits — not the picker's default state.
        sync_picker_from_slot(&mut e.crosshair_ui, &e.crosshair);
        e
    }

    /// Read-only crosshair config — the JNI layer uses this to answer
    /// `nativeIsCustomCrosshairEnabled` for the Java mixin.
    pub fn crosshair_config(&self) -> &crate::crosshair::CrosshairConfig {
        &self.crosshair
    }

    /// Select a dashboard view by its tab label (case-insensitive), e.g.
    /// `"home"`, `"modules"`, `"pvp"`. Returns `false` for an unknown name.
    ///
    /// In-game the view is only ever changed by clicking the tab strip, which
    /// needs a cursor and a live window — so the offscreen render harness
    /// ([`examples/hudshot.rs`]) drives it through here instead.
    pub fn set_view_by_name(&mut self, name: &str) -> bool {
        let Some(view) = OverlayView::ALL
            .into_iter()
            .find(|v| v.title().eq_ignore_ascii_case(name))
        else {
            return false;
        };
        self.view = view;
        // Match the tab-strip click path: a fresh view always starts scrolled
        // to the top, with no popover carried over from the previous tab.
        self.modules_scroll = 0.0;
        self.module_popover = None;
        true
    }

    /// Every selectable view name, in tab-strip order — so the harness can
    /// enumerate tabs without duplicating the list.
    pub fn view_names() -> Vec<&'static str> {
        OverlayView::ALL.into_iter().map(|v| v.title()).collect()
    }

    /// Park the cursor at `(x, y)` without running the hover/drag logic in
    /// [`Self::on_mouse_move`] — the harness uses this to render hover states
    /// deterministically.
    pub fn set_cursor(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
    }

    /// Look up a widget by its display title (case-insensitive), e.g. `"FPS"`,
    /// `"TARGET"`. `None` for an unknown name.
    pub(super) fn widget_by_title(title: &str) -> Option<WidgetId> {
        WidgetId::ALL
            .into_iter()
            .find(|id| id.title().eq_ignore_ascii_case(title))
    }

    /// Set a widget's size multiplier. Returns `false` for an unknown name.
    ///
    /// In-game this is reached by dragging the resize grip; the offscreen
    /// harness has no cursor, so it drives the same state through here.
    pub fn set_widget_scale(&mut self, title: &str, scale: f32) -> bool {
        let Some(id) = Self::widget_by_title(title) else {
            return false;
        };
        self.layout.get_mut(id).scale = scale.clamp(SCALE_MIN, SCALE_MAX);
        true
    }

    /// Select a widget, as clicking it in the HUD editor would — which is what
    /// makes its resize grip appear.
    pub fn select_widget(&mut self, title: &str) -> bool {
        let Some(id) = Self::widget_by_title(title) else {
            return false;
        };
        self.selected = Some(id);
        true
    }

    /// Every widget's display title, for harness enumeration + error messages.
    pub fn widget_titles() -> Vec<&'static str> {
        WidgetId::ALL.into_iter().map(|id| id.title()).collect()
    }

    /// Inject a fixed "now playing" state.
    ///
    /// Media normally arrives from the SMTC backend, which makes any media
    /// shot depend on whatever happens to be playing on the machine at the
    /// time — no good for a harness that is supposed to be deterministic, and
    /// no good at all on a machine with nothing playing. This pins it.
    pub fn set_media(
        &mut self,
        title: &str,
        artist: &str,
        playing: bool,
        position_seconds: f32,
        duration_seconds: f32,
    ) {
        self.media.title = title.to_string();
        self.media.artist = artist.to_string();
        self.media.playing = playing;
        self.media.position_seconds = position_seconds;
        self.media.duration_seconds = duration_seconds;
        self.media.source = "HARNESS".to_string();
        // Anchor both clocks to now so `displayed_position` does not drift the
        // scrub head forward between the injection and the draw.
        let t = std::time::Instant::now();
        self.media.position_updated_at = t;
        self.media.title_changed_at = t;
        // Otherwise the very next `draw` drains a real SMTC update over the
        // top of this and the pin lasts zero frames.
        self.media_pinned = true;
    }

    /// Pin a synthetic spectrum for the offscreen harness.
    ///
    /// `shape` seeds a plausible mix — bass-heavy, rolling off toward the
    /// treble, with a little per-band variation — rather than a flat row of
    /// identical bars, which would hide exactly the layout bugs a screenshot
    /// is meant to catch.
    pub fn set_spectrum(&mut self, level: f32, pulse: f32, shape: f32) {
        let mut bands = [0.0f32; crate::audio::BANDS];
        for (i, b) in bands.iter_mut().enumerate() {
            let t = i as f32 / (crate::audio::BANDS - 1) as f32;
            // Falling tilt plus two standing bumps — reads like a real mix.
            let tilt = 1.0 - 0.55 * t;
            let bump = 0.22 * ((t * 9.0 + shape).sin() * 0.5 + 0.5);
            *b = ((tilt + bump) * level).clamp(0.0, 1.0);
        }
        self.spectrum = crate::audio::Spectrum {
            bands,
            level,
            pulse,
            source: crate::audio::Source::ExcludingGame,
        };
        self.spectrum_pinned = true;
    }

    /// Rects (framebuffer pixels) where the composite step should leave
    /// the un-frosted live game visible. Currently only fires on the
    /// CROSSHAIR view, returning the 3 preview-pane rects — the
    /// composite step re-blits the pre-frost snapshot back into them so
    /// the crosshair previews show the real game at 1:1 (true-to-life
    /// peek). Other views return empty.
    pub fn live_game_cutouts(&self, w: f32, h: f32) -> Vec<Rect> {
        if self.view != OverlayView::Crosshair {
            return Vec::new();
        }
        let layout = crosshair_layout(w, h);
        vec![layout.preview_idle, layout.preview_target, layout.preview_reach]
    }

    /// Human-readable label for the keybind bound to `module_id`, or `None` if
    /// the module is unbound or the keybind file wasn't available at boot. Used
    /// by the MODULES tab to render a small chip on the right of each row.
    pub(super) fn keybind_label_for(&self, module_id: &str) -> Option<String> {
        let code = self
            .keybinds
            .iter()
            .find(|(id, _)| id == module_id)
            .map(|&(_, c)| c)?;
        if code == 0 {
            None
        } else {
            Some(glfw_key_label(code))
        }
    }

    /// The HUD paint-rate cap, chosen in the settings view.
    pub fn paint_rate(&self) -> crate::HudPaintRate {
        self.layout.paint_rate
    }

    /// Liquid-glass intensity from the overlay SETTINGS tab.
    pub fn glass_strength(&self) -> f32 {
        self.layout.glass_strength
    }

    /// Enter or leave quick-edit — the modifier-held state over a vanilla
    /// screen. Called from Java each frame.
    ///
    /// Leaving mid-drag commits whatever is in progress rather than abandoning
    /// it: the user let go of the modifier, they did not undo. Dropping the
    /// drag here would also strand `dragging`/`resizing` set, so the next
    /// unrelated cursor move would keep dragging a widget.
    pub fn set_quick_edit(&mut self, on: bool) {
        if self.quick_edit == on {
            return;
        }
        self.quick_edit = on;
        if !on {
            let had_drag = self.dragging.take().is_some() | self.resizing.take().is_some();
            self.snap_x = None;
            self.snap_y = None;
            if had_drag {
                self.layout.save();
            }
        }
    }

    /// Whether quick-edit is active *and* usable — the overlay being open
    /// means the real editor is available and takes precedence.
    pub(super) fn quick_edit_active(&self) -> bool {
        self.quick_edit && !self.overlay_open
    }

    /// Whether the current view wants the live game frosted behind it. The
    /// data views (Mods / Settings) do — for a real glass-over-depth backdrop;
    /// the HUD editor doesn't, so widgets stay readable against the game.
    pub fn frosts_game(&self) -> bool {
        !matches!(self.view, OverlayView::HudEditor)
    }

    /// Switch the active client profile — persist it to `profiles.toml`
    /// and reload the HUD layout from the new profile's `hud.toml`, live.
    pub(super) fn switch_profile(&mut self, name: String) {
        if name == self.active_profile {
            return;
        }
        write_profiles(&name, &self.profiles);
        self.active_profile = name;
        self.layout = HudLayout::load();
        // Modules are per-profile too — reload them for the switched-to profile.
        self.modules = crate::modules::ModuleConfig::load();
        // Crosshair is per-profile as well — reload `<profile>/crosshair.toml`
        // so the new profile's saved shape + colours apply live; then snap
        // the HSV picker to the freshly-loaded selected swatch so the UI
        // doesn't carry the previous profile's picker position.
        self.crosshair = crate::crosshair::load();
        sync_picker_from_slot(&mut self.crosshair_ui, &self.crosshair);
    }

    /// Cursor moved — drag the active widget if one is held, snapping its
    /// edges/centres to other widgets' for a gentle alignment assist.
    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        // HOME 3D-skin drag — rotate the model by the horizontal delta.
        if let Some(last_x) = self.skin_drag {
            self.skin_yaw += (x - last_x) * 0.012;
            self.skin_drag = Some(x);
            return;
        }
        // MODULES-view slider drag — track the cursor to the setting value.
        if let Some((idx, slot)) = self.slider_drag {
            self.drag_module_slider(idx, slot, x);
            return;
        }
        // PVP-view slider drag — same idea, but the slot identifies which
        // PVP control is held.
        if let Some(drag) = self.pvp_drag {
            self.drag_pvp_slider(drag, x);
            return;
        }
        // CROSSHAIR-view slider drag — slot identifies which control is
        // held. The 2D SV square uses both coordinates; the 1D sliders
        // ignore `y` (or `x`, for the vertical hue/alpha strips).
        if let Some(slot) = self.crosshair_ui.drag {
            self.drag_crosshair_slider(slot, x, y);
            return;
        }
        // HUD-editor corner resize. Scale follows the cursor's distance from
        // the anchor relative to where it was grabbed, so the widget's corner
        // stays under the pointer.
        if let Some(rs) = self.resizing {
            let (w, h) = self.window;
            let wl = self.layout.get(rs.id);
            let (ax, ay) = (wl.x * w, wl.y * h);
            let dist = ((x - ax).powi(2) + (y - ay).powi(2)).sqrt();
            let scale = rs.start_scale * (dist / rs.grab_dist.max(1.0));
            self.layout.get_mut(rs.id).scale = scale.clamp(SCALE_MIN, SCALE_MAX);
            return;
        }
        // SETTINGS-view glass-strength drag. Tracks `x` even once the cursor
        // leaves the track vertically — the usual slider grab behaviour.
        if self.glass_drag {
            let (_, _, _, glass) =
                settings_layout(self.window.0, self.window.1, self.profiles.len());
            self.layout.glass_strength = glass_value_at(glass, x);
            return;
        }
        let Some(drag) = &self.dragging else {
            return;
        };
        let drag_id = drag.id;
        let grab_dx = drag.grab_dx;
        let grab_dy = drag.grab_dy;
        let (w, h) = self.window;
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        let wl = self.layout.get(drag_id);
        let dragged = self.bounds[drag_id.index()];
        let (ww, wh) = (dragged.width(), dragged.height());

        // Proposed anchor point (window pixels), before snapping.
        let mut ap_x = x - grab_dx;
        let mut ap_y = y - grab_dy;
        self.snap_x = None;
        self.snap_y = None;

        // Gather every other visible widget's edge + centre lines and snap the
        // dragged widget's own edges/centre onto the nearest within SNAP_PX.
        if ww > 0.0 && wh > 0.0 {
            let mut fixed_x: Vec<f32> = Vec::new();
            let mut fixed_y: Vec<f32> = Vec::new();
            for id in WidgetId::ALL {
                if id == drag_id {
                    continue;
                }
                let b = self.bounds[id.index()];
                if b.width() <= 0.0 {
                    continue;
                }
                fixed_x.push(b.left);
                fixed_x.push((b.left + b.right) * 0.5);
                fixed_x.push(b.right);
                fixed_y.push(b.top);
                fixed_y.push((b.top + b.bottom) * 0.5);
                fixed_y.push(b.bottom);
            }
            let (tlx, tly) = wl.anchor.origin(ap_x, ap_y, ww, wh);
            if let Some((dx, line)) = nearest_snap([tlx, tlx + ww * 0.5, tlx + ww], &fixed_x) {
                ap_x += dx;
                self.snap_x = Some(line);
            }
            if let Some((dy, line)) = nearest_snap([tly, tly + wh * 0.5, tly + wh], &fixed_y) {
                ap_y += dy;
                self.snap_y = Some(line);
            }
        }

        let wl = self.layout.get_mut(drag_id);
        wl.x = (ap_x / w).clamp(0.0, 1.0);
        wl.y = (ap_y / h).clamp(0.0, 1.0);
    }

    /// Mouse button — `button` is the GLFW code (`0` = left, `1` = right). A
    /// left press first checks the view-tab strip, then routes to the active
    /// view; a release ends + persists a drag. Right-click opens the MODULES
    /// settings popover (no-op elsewhere for now).
    pub fn on_mouse_button(&mut self, button: i32, pressed: bool, x: f32, y: f32) {
        self.cursor = (x, y);

        // Right-click: settings popover on MODULES rows that have sliders.
        // Press only — releases of the right button do nothing.
        if button == 1 {
            if !pressed {
                return;
            }
            if self.view == OverlayView::Modules {
                let (panel, _, rows, content_h) =
                    modules_layout(self.window.0, self.window.1);
                let body_top = panel.top + 24.0 + 76.0;
                let visible = panel.bottom - body_top - 8.0;
                let max_scroll = (content_h - visible).max(0.0);
                let scroll_y = self.modules_scroll.clamp(0.0, max_scroll);
                let logical_y = y + scroll_y;
                for row in rows.iter() {
                    if !catalog::REGISTRY[row.catalog_index].settings.is_empty()
                        && point_in(row.row, x, logical_y)
                    {
                        self.module_popover = Some(row.catalog_index);
                        return;
                    }
                }
            }
            return;
        }

        // Anything below this point handles the LEFT mouse button.
        if !pressed {
            self.skin_drag = None;
            if self.slider_drag.take().is_some() {
                // A module-slider drag finished — persist the setting now.
                self.modules.save();
            }
            if self.pvp_drag.take().is_some() {
                // A PVP-tab slider drag finished — persist the config.
                self.pvp.save();
            }
            if self.crosshair_ui.drag.take().is_some() {
                // A crosshair-tab slider drag finished — clamp + persist.
                self.crosshair.clamp();
                crate::crosshair::save(&self.crosshair);
            }
            if std::mem::take(&mut self.glass_drag) {
                // A glass-strength drag finished — persist to `hud.toml`.
                self.layout.save();
            }
            if self.resizing.take().is_some() {
                // A resize finished — persist the new scale.
                self.layout.save();
            }
            if self.dragging.take().is_some() {
                // A drag finished — drop the snap guides and persist.
                self.snap_x = None;
                self.snap_y = None;
                self.layout.save();
            }
            return;
        }

        // While the module-settings popover is open, all left clicks belong
        // to it: hits inside route to its sliders / close button; hits
        // outside close it and the click is consumed (no fall-through to the
        // view dispatch).
        if let Some(idx) = self.module_popover {
            let layout = module_popover_layout(self.window.0, self.window.1, idx);
            if point_in(layout.card, x, y) {
                // Close button (top-right of the card).
                if point_in(layout.close, x, y) {
                    self.module_popover = None;
                    return;
                }
                // Slider press → start a drag on that setting.
                for (slot, &track) in layout.sliders.iter().enumerate() {
                    if point_in(track, x, y) {
                        self.slider_drag = Some((idx, slot));
                        self.drag_module_slider(idx, slot, x);
                        return;
                    }
                }
                // Click on the popover background — absorb the click so we
                // don't accidentally close on margins inside the card.
                return;
            }
            // Click outside the card — close the popover and absorb.
            self.module_popover = None;
            return;
        }

        // The top-centre view-tab strip takes priority.
        let (_, tabs) = tab_layout(self.window.0);
        for (i, &tab) in tabs.iter().enumerate() {
            if point_in(tab, x, y) {
                self.view = OverlayView::ALL[i];
                // Reset scroll on view switch so re-entering MODULES always
                // starts at the top, not mid-list.
                self.modules_scroll = 0.0;
                self.module_popover = None;
                return;
            }
        }

        // Quick-edit intercepts the press before any view dispatch: the
        // overlay is closed, so `self.view` is whatever tab the user last had
        // open and is meaningless here.
        if self.quick_edit_active() {
            self.editor_press(x, y);
            return;
        }

        match self.view {
            OverlayView::Home => {
                let (_, skin_rect, _, media_rect, _) =
                    home_layout(self.window.0, self.window.1);
                if point_in(skin_rect, x, y) {
                    self.skin_drag = Some(x);
                    return;
                }
                // Media-controller transport buttons (prev / play / next).
                // Geometry must mirror `draw_media_large`'s control layout.
                if point_in(media_rect, x, y) {
                    let ctl_size = 34.0;
                    let play_size = 46.0;
                    let gap = 8.0;
                    let cluster_w = ctl_size + gap + play_size + gap + ctl_size;
                    let cluster_left = media_rect.right - 14.0 - cluster_w;
                    let cy = media_rect.top + media_rect.height() * 0.5;
                    let prev = Rect::from_xywh(
                        cluster_left,
                        cy - ctl_size * 0.5,
                        ctl_size,
                        ctl_size,
                    );
                    let play = Rect::from_xywh(
                        cluster_left + ctl_size + gap,
                        cy - play_size * 0.5,
                        play_size,
                        play_size,
                    );
                    let next = Rect::from_xywh(
                        cluster_left + ctl_size + gap + play_size + gap,
                        cy - ctl_size * 0.5,
                        ctl_size,
                        ctl_size,
                    );
                    if point_in(prev, x, y) {
                        self.media_service.act(crate::media::MediaAction::Previous);
                        self.media_button_press = Some((0, std::time::Instant::now()));
                        return;
                    }
                    if point_in(play, x, y) {
                        self.media_service.act(crate::media::MediaAction::PlayPause);
                        self.media_button_press = Some((1, std::time::Instant::now()));
                        return;
                    }
                    if point_in(next, x, y) {
                        self.media_service.act(crate::media::MediaAction::Next);
                        self.media_button_press = Some((2, std::time::Instant::now()));
                        return;
                    }
                }
                // Chips are flowed at render time; their rects live in
                // `home_toggle_bounds` (populated by draw_home each frame).
                for (i, &tog) in self.home_toggle_bounds.iter().enumerate() {
                    if point_in(tog, x, y) {
                        let wl = self.layout.get_mut(WidgetId::ALL[i]);
                        wl.enabled = !wl.enabled;
                        self.layout.save();
                        return;
                    }
                }
            }
            OverlayView::HudEditor => self.editor_press(x, y),
            OverlayView::Modules => {
                let (panel, _, rows, content_h) = modules_layout(self.window.0, self.window.1);
                // The panel header sits above the scroll region; scroll-aware
                // hit-tests only apply inside the body rect.
                let body_top = panel.top + 24.0 + 76.0;
                let visible = panel.bottom - body_top - 8.0;
                let max_scroll = (content_h - visible).max(0.0);
                let scroll_y = self.modules_scroll.clamp(0.0, max_scroll);
                // Rows are laid out in logical (un-scrolled) coords; shift the
                // click by +scroll_y to match.
                let logical_y = y + scroll_y;
                for row in rows.iter() {
                    let idx = row.catalog_index;
                    if point_in(row.toggle, x, logical_y) {
                        self.modules.toggle(idx);
                        return;
                    }
                    for (slot, &track) in row.sliders.iter().enumerate() {
                        if point_in(track, x, logical_y) {
                            self.slider_drag = Some((idx, slot));
                            self.drag_module_slider(idx, slot, x);
                            return;
                        }
                    }
                }
            }
            OverlayView::Mods => {
                let (_, toggles) = mods_layout(self.window.0, self.window.1, self.mods.len());
                for (i, &toggle) in toggles.iter().enumerate() {
                    if point_in(toggle, x, y) {
                        self.mods[i].enabled = !self.mods[i].enabled;
                        save_mod_overrides(&self.mods);
                        return;
                    }
                }
            }
            OverlayView::Settings => {
                let (_, chips, buttons, glass) =
                    settings_layout(self.window.0, self.window.1, self.profiles.len());
                for (i, &chip) in chips.iter().enumerate() {
                    if point_in(chip, x, y) {
                        if let Some(name) = self.profiles.get(i).cloned() {
                            self.switch_profile(name);
                        }
                        return;
                    }
                }
                for (i, &btn) in buttons.iter().enumerate() {
                    if point_in(btn, x, y) {
                        self.layout.paint_rate = crate::HudPaintRate::ALL[i];
                        self.layout.save();
                        return;
                    }
                }
                if point_in(glass, x, y) {
                    // Jump to the pressed value, then track the cursor until
                    // release — same grab model as the PVP-tab sliders.
                    self.layout.glass_strength = glass_value_at(glass, x);
                    self.glass_drag = true;
                    return;
                }
            }
            OverlayView::Pvp => self.pvp_press(x, y),
            OverlayView::Crosshair => self.crosshair_press(x, y),
            // FRIENDS is read-only (view who's online). Mutations + join stay
            // on the launcher side for now.
            OverlayView::Friends => {}
        }
    }

    /// Handle a press in the HUD-editor view — the side panel or widget drag.
    pub(super) fn editor_press(&mut self, x: f32, y: f32) {
        // Quick-edit has no side panel — the player is standing in their
        // inventory, not the HUD editor — so skip straight to the widgets.
        if self.quick_edit && !self.overlay_open {
            self.widget_press(x, y);
            return;
        }

        let panel = panel_layout(self.window.1);

        // A widget's enable toggle.
        for (i, &toggle) in panel.toggles.iter().enumerate() {
            if point_in(toggle, x, y) {
                let wl = self.layout.get_mut(WidgetId::ALL[i]);
                wl.enabled = !wl.enabled;
                self.layout.save();
                return;
            }
        }
        // A widget row — select it (so the anchor grid targets it).
        for (i, &row) in panel.rows.iter().enumerate() {
            if point_in(row, x, y) {
                self.selected = Some(WidgetId::ALL[i]);
                return;
            }
        }
        // An anchor preset cell — jump the selected widget to that corner.
        if let Some(sel) = self.selected {
            for (i, &cell) in panel.cells.iter().enumerate() {
                if point_in(cell, x, y) {
                    let (anchor, px, py) = ANCHOR_PRESETS[i];
                    let wl = self.layout.get_mut(sel);
                    wl.anchor = anchor;
                    wl.x = px;
                    wl.y = py;
                    self.layout.save();
                    return;
                }
            }
        }
        // A press anywhere else inside the panel is swallowed — no drag.
        if point_in(panel.panel, x, y) {
            return;
        }

        self.widget_press(x, y);
    }

    /// Grab the widget (or resize grip) under `(x, y)`. Shared by the HUD
    /// editor tab and quick-edit, which differ only in whether a side panel
    /// gets first refusal on the press.
    pub(super) fn widget_press(&mut self, x: f32, y: f32) {
        let (w, h) = self.window;

        // The selected widget's resize handle. Checked before the move test
        // because the handle straddles the widget's corner: a press inside it
        // is inside the widget's bounds too, and whichever is tested first
        // wins. Only the *selected* widget shows a handle, so at most one can
        // ever match.
        if let Some(id) = self.selected {
            let b = self.bounds[id.index()];
            let wl = self.layout.get(id);
            if b.width() > 0.0 && point_in(resize_handle_rect(b, wl.anchor), x, y) {
                let (ax, ay) = (wl.x * w, wl.y * h);
                let dist = ((x - ax).powi(2) + (y - ay).powi(2)).sqrt();
                // A grab right on the anchor point would divide by ~0 and send
                // the scale to infinity on the first pixel of movement.
                if dist > 4.0 {
                    self.resizing = Some(ResizeDrag {
                        id,
                        start_scale: wl.scale,
                        grab_dist: dist,
                    });
                    return;
                }
            }
        }

        for id in WidgetId::ALL {
            let b = self.bounds[id.index()];
            if b.width() > 0.0 && point_in(b, x, y) {
                let wl = self.layout.get(id);
                self.selected = Some(id);
                self.dragging = Some(Drag {
                    id,
                    grab_dx: x - wl.x * w,
                    grab_dy: y - wl.y * h,
                });
                break;
            }
        }
    }

    /// Track a MODULES-view slider drag: map the cursor `x` to the setting
    /// value and apply it. The value persists on drag-release, not per-move.
    /// Mouse click in the in-world Media widget — called from the Fabric mod
    /// when a vanilla screen (inventory, pause, chat, …) is open and the user
    /// left-clicks anywhere. Returns `true` if the click hit one of the
    /// widget's prev / play / next transport buttons and an action was
    /// dispatched, so the caller can cancel the press to keep the vanilla
    /// screen from also receiving it.
    ///
    /// Geometry must match `draw_media_compact`'s control cluster one-to-one;
    /// the widget's overall bounds are read from `editor.bounds`, which the
    /// renderer fills every frame.
    pub fn try_media_click(&mut self, button: i32, x: f32, y: f32) -> bool {
        // Only left-click triggers transport actions.
        if button != 0 {
            return false;
        }
        // The widget must actually be drawn this frame.
        let wl = self.layout.get(WidgetId::Media);
        if !wl.enabled {
            return false;
        }
        let bounds = self.bounds[WidgetId::Media.index()];
        if bounds.width() <= 0.0 || !point_in(bounds, x, y) {
            return false;
        }

        // Match `draw_media_compact` — three 22px circles, the rightmost
        // cluster of the widget, with 4px gaps and a 12px right pad.
        let btn = 22.0;
        let btn_gap = 4.0;
        let pad_r = 12.0;
        let cluster_w = btn * 3.0 + btn_gap * 2.0;
        let cluster_left = bounds.right - pad_r - cluster_w;
        let cy = bounds.top + bounds.height() * 0.5;
        let prev = Rect::from_xywh(cluster_left, cy - btn * 0.5, btn, btn);
        let play = Rect::from_xywh(cluster_left + btn + btn_gap, cy - btn * 0.5, btn, btn);
        let next = Rect::from_xywh(
            cluster_left + (btn + btn_gap) * 2.0,
            cy - btn * 0.5,
            btn,
            btn,
        );
        if point_in(prev, x, y) {
            self.media_service.act(crate::media::MediaAction::Previous);
            self.media_button_press = Some((0, std::time::Instant::now()));
            return true;
        }
        if point_in(play, x, y) {
            self.media_service.act(crate::media::MediaAction::PlayPause);
            self.media_button_press = Some((1, std::time::Instant::now()));
            return true;
        }
        if point_in(next, x, y) {
            self.media_service.act(crate::media::MediaAction::Next);
            self.media_button_press = Some((2, std::time::Instant::now()));
            return true;
        }
        false
    }

    /// Scroll wheel — currently only the MODULES tab consumes it. Each "notch"
    /// from `nativeMouseScroll` is roughly ±1; we want ~one row per notch so
    /// the list scrolls predictably.
    pub fn on_scroll(&mut self, dy: f32) {
        if self.view != OverlayView::Modules {
            return;
        }
        const PIXELS_PER_NOTCH: f32 = 70.0;
        self.modules_scroll = (self.modules_scroll - dy * PIXELS_PER_NOTCH).max(0.0);
        // The renderer clamps to max_scroll for us each frame; no need to
        // recompute the layout here for the upper bound.
    }

    pub(super) fn drag_module_slider(&mut self, idx: usize, slot: usize, x: f32) {
        // Sliders only live in the popover now; if it isn't the one we're
        // dragging (or it's not open), there's nothing to update.
        let Some(open_idx) = self.module_popover else {
            return;
        };
        if open_idx != idx {
            return;
        }
        let layout = module_popover_layout(self.window.0, self.window.1, idx);
        let Some(track) = layout.sliders.get(slot).copied() else {
            return;
        };
        let Some(setting) = catalog::REGISTRY.get(idx).and_then(|m| m.settings.get(slot))
        else {
            return;
        };
        // Sliders have a small horizontal margin inside the track rect — use
        // the inner span (matches `draw_module_slider`'s knob travel).
        let inner_left = track.left + 14.0;
        let inner_right = track.right - 54.0;
        let span = (inner_right - inner_left).max(1.0);
        let frac = ((x - inner_left) / span).clamp(0.0, 1.0);
        let mut value = setting.min + frac * (setting.max - setting.min);
        if setting.step > 0.0 {
            value = (value / setting.step).round() * setting.step;
        }
        self.modules.set_setting(idx, slot, value);
    }

    /// The widget the cursor is over (the one being dragged always wins).
    pub(super) fn active_widget(&self) -> Option<WidgetId> {
        if let Some(drag) = &self.dragging {
            return Some(drag.id);
        }
        WidgetId::ALL.into_iter().find(|id| {
            let b = self.bounds[id.index()];
            b.width() > 0.0 && point_in(b, self.cursor.0, self.cursor.1)
        })
    }

    /// PVP tab — a press cycles a sound chip, flips a toggle, or starts a
    /// slider drag. Edits are persisted to `pvp.toml` on commit (toggle / chip
    /// click immediately; slider drag on release in `on_mouse_button`).
    pub(super) fn pvp_press(&mut self, x: f32, y: f32) {
        let layout = pvp_layout(self.window.0, self.window.1);

        // General-section toggles.
        for (i, &rect) in layout.general_toggles.iter().enumerate() {
            if point_in(rect, x, y) {
                match i {
                    0 => self.pvp.jump_reset_enabled = !self.pvp.jump_reset_enabled,
                    1 => self.pvp.jump_reset_bar_enabled = !self.pvp.jump_reset_bar_enabled,
                    2 => self.pvp.hit_range_enabled = !self.pvp.hit_range_enabled,
                    3 => self.pvp.totem_count_enabled = !self.pvp.totem_count_enabled,
                    4 => self.pvp.floating_health_enabled = !self.pvp.floating_health_enabled,
                    _ => {}
                }
                self.pvp.save();
                return;
            }
        }

        // Sound-cycle chips for each tier.
        for (i, &rect) in layout.tier_sound.iter().enumerate() {
            if point_in(rect, x, y) {
                let tier = crate::pvp::Tier::ALL[i];
                let slot = self.pvp.sound_for_tier_mut(tier);
                let next = (slot.sound.index() + 1) % crate::pvp::PvpSound::ALL.len();
                slot.sound = crate::pvp::PvpSound::ALL[next];
                self.pvp.save();
                return;
            }
        }

        // Tier volume sliders.
        for (i, &rect) in layout.tier_volume.iter().enumerate() {
            if point_in(rect, x, y) {
                self.pvp_drag = Some(PvpDrag::TierVolume(i));
                self.drag_pvp_slider(PvpDrag::TierVolume(i), x);
                return;
            }
        }

        // Zone enable toggles + min/max sliders + sound chips + volume sliders.
        for i in 0..3 {
            if point_in(layout.zone_enable[i], x, y) {
                let z = self.pvp.zone_mut(i);
                z.enabled = !z.enabled;
                self.pvp.save();
                return;
            }
            if point_in(layout.zone_min[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneMinDist(i));
                self.drag_pvp_slider(PvpDrag::ZoneMinDist(i), x);
                return;
            }
            if point_in(layout.zone_max[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneMaxDist(i));
                self.drag_pvp_slider(PvpDrag::ZoneMaxDist(i), x);
                return;
            }
            if point_in(layout.zone_sound[i], x, y) {
                let z = self.pvp.zone_mut(i);
                let next = (z.sound.index() + 1) % crate::pvp::PvpSound::ALL.len();
                z.sound = crate::pvp::PvpSound::ALL[next];
                self.pvp.save();
                return;
            }
            if point_in(layout.zone_volume[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneVolume(i));
                self.drag_pvp_slider(PvpDrag::ZoneVolume(i), x);
                return;
            }
        }
    }

    /// Track a PVP slider drag — the slot identifies which control is held
    /// (the cursor x maps to its value). Persists on drag-release, not
    /// per-frame, so dragging doesn't write `pvp.toml` 60 times a second.
    pub(super) fn drag_pvp_slider(&mut self, slot: PvpDrag, x: f32) {
        let layout = pvp_layout(self.window.0, self.window.1);
        let frac_of = |track: Rect| -> f32 {
            let span = (track.right - track.left - 44.0).max(1.0); // value strip
            ((x - track.left - 4.0) / span).clamp(0.0, 1.0)
        };
        match slot {
            PvpDrag::TierVolume(i) => {
                let frac = frac_of(layout.tier_volume[i]);
                let tier = crate::pvp::Tier::ALL[i];
                self.pvp.sound_for_tier_mut(tier).volume = frac;
            }
            PvpDrag::ZoneMinDist(i) => {
                let frac = frac_of(layout.zone_min[i]);
                let z = self.pvp.zone_mut(i);
                z.min_dist = (frac * 3.5).max(0.0).min(z.max_dist - 0.05);
            }
            PvpDrag::ZoneMaxDist(i) => {
                let frac = frac_of(layout.zone_max[i]);
                let z = self.pvp.zone_mut(i);
                z.max_dist = (frac * 3.5).max(z.min_dist + 0.05).min(3.5);
            }
            PvpDrag::ZoneVolume(i) => {
                let frac = frac_of(layout.zone_volume[i]);
                self.pvp.zone_mut(i).volume = frac;
            }
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// `(x, y)` lies inside `r`.
pub(super) fn point_in(r: Rect, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

/// Find the smallest shift that aligns one of the `moving` lines onto one of
/// the `fixed` lines, within [`SNAP_PX`]. Returns `(shift, snapped-onto line)`.
pub(super) fn nearest_snap(moving: [f32; 3], fixed: &[f32]) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    let mut best_dist = SNAP_PX;
    for &m in &moving {
        for &f in fixed {
            let shift = f - m;
            if shift.abs() < best_dist {
                best_dist = shift.abs();
                best = Some((shift, f));
            }
        }
    }
    best
}

// ────────────────────────────────────────────────────────────────────────
// Full-HUD dispatch.
// ────────────────────────────────────────────────────────────────────────
