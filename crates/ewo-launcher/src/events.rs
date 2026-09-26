//! The launcher window's event handlers, one method per large
//! `WindowEvent` arm. `App::window_event` (`main.rs`) is the dispatcher.

use super::*;

impl App {
    /// A key press (`WindowEvent::KeyboardInput`, pressed).
    pub(crate) fn on_key_pressed(&mut self, window: &Arc<Window>, logical_key: Key, physical_key: winit::keyboard::PhysicalKey, text: Option<winit::keyboard::SmolStr>) {
        // Keybind capture — the Keybinds tab armed a rebind, so the
        // next key press becomes the new binding. Esc cancels; a key
        // GLFW can't name leaves the rebind armed.
        if let Some(action_id) = self.keybind_capture.clone() {
            if logical_key == Key::Named(NamedKey::Escape) {
                log::info!("keybind: capture cancelled");
                self.keybind_capture = None;
            } else if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                if let Some(chord) = keybind::KeyChord::from_winit(code) {
                    log::info!("keybind: {} → {}", action_id, chord.label());
                    self.keybinds.insert(action_id, chord);
                    profile::save_keybinds(&self.keybinds);
                    self.keybind_capture = None;
                }
            }
            window.request_redraw();
            return;
        }
        if self.instance_prefs.renaming {
            // Inline rename for the selected instance.
            const RENAME_MAX_LEN: usize = 48;
            if logical_key == Key::Named(NamedKey::Escape) {
                log::info!("rename: Esc → cancelled");
                self.instance_prefs.renaming = false;
                self.instance_prefs.rename_buffer.clear();
            } else if logical_key == Key::Named(NamedKey::Enter) {
                // Display name only — the folder is keyed by the
                // instance's stable `id`, so a rename moves nothing.
                match persistence::validate_instance_name(
                    &self.instance_prefs.rename_buffer,
                ) {
                    Ok(trimmed) => {
                        let trimmed = trimmed.to_string();
                        if let Some(inst) =
                            self.instances.get_mut(self.instance_prefs.selected)
                        {
                            log::info!(
                                "rename: \"{}\" → \"{}\"",
                                inst.name, trimmed
                            );
                            inst.name = trimmed;
                        }
                        persistence::save_instances(&self.instances);
                    }
                    Err(e) => log::info!("rename: rejected ({:?}) → cancelled", e),
                }
                self.instance_prefs.renaming = false;
                self.instance_prefs.rename_buffer.clear();
            } else if logical_key == Key::Named(NamedKey::Backspace) {
                self.instance_prefs.rename_buffer.pop();
                self.instance_prefs.rename_focus_time = 0.0;
            } else if let Some(t) = text {
                for ch in t.chars() {
                    if !ch.is_control()
                        && self.instance_prefs.rename_buffer.chars().count() < RENAME_MAX_LEN
                    {
                        self.instance_prefs.rename_buffer.push(ch);
                        self.instance_prefs.rename_focus_time = 0.0;
                    }
                }
            }
        } else if self.prefs.profile_renaming.is_some() {
            // Inline rename for a client profile (Settings → Profiles).
            if logical_key == Key::Named(NamedKey::Escape) {
                log::info!("profile rename: Esc → cancelled");
                self.prefs.profile_renaming = None;
                self.prefs.profile_rename_buffer.clear();
            } else if logical_key == Key::Named(NamedKey::Enter) {
                if let Some(idx) = self.prefs.profile_renaming {
                    self.prefs.profile_request = Some(ProfileRequest::Rename {
                        index: idx,
                        new_name: self.prefs.profile_rename_buffer.clone(),
                    });
                }
                self.prefs.profile_renaming = None;
            } else if logical_key == Key::Named(NamedKey::Backspace) {
                self.prefs.profile_rename_buffer.pop();
                self.prefs.profile_rename_focus_time = 0.0;
            } else if let Some(t) = text {
                for ch in t.chars() {
                    // Skip control + path-unsafe chars — the name
                    // becomes a directory under `profiles/`.
                    if !ch.is_control()
                        && !"/\\:*?\"<>|".contains(ch)
                        && self.prefs.profile_rename_buffer.chars().count()
                            < profile::MAX_NAME_LEN
                    {
                        self.prefs.profile_rename_buffer.push(ch);
                        self.prefs.profile_rename_focus_time = 0.0;
                    }
                }
            }
        } else if logical_key == Key::Named(NamedKey::Escape) && self.about_modal.open {
            log::info!("about: Esc → closing");
            self.about_modal.close();
        } else if self.launcher_link_modal.open {
            // Phase H2 launcher-link modal — captures Esc, Enter,
            // Backspace, and digit chars while open. Non-digit
            // chars are ignored (push_digit double-checks).
            if logical_key == Key::Named(NamedKey::Escape) {
                log::info!("launcher-link modal: Esc → closing");
                self.launcher_link_modal.close();
                self.social.clear_link_redeem();
            } else if logical_key == Key::Named(NamedKey::Backspace) {
                self.launcher_link_modal.pop_digit();
            } else if logical_key == Key::Named(NamedKey::Enter)
                && self.launcher_link_modal.is_ready()
            {
                let code = self.launcher_link_modal.code.clone();
                log::info!("launcher-link modal: Enter → submitting");
                self.social.submit_link_code(code);
            } else if let Some(t) = text.as_ref() {
                for ch in t.chars() {
                    if ch.is_ascii_digit() {
                        self.launcher_link_modal.push_digit(ch);
                    }
                }
            }
        } else if self.screen == Screen::Friends
            && self.friends_prefs.add_focused
        {
            // Phase H5 — text input for the add-friend MC name.
            const NAME_MAX_LEN: usize = 32;
            if logical_key == Key::Named(NamedKey::Escape) {
                self.friends_prefs.add_focused = false;
            } else if logical_key == Key::Named(NamedKey::Backspace) {
                self.friends_prefs.add_buffer.pop();
                self.friends_prefs.add_focus_time = 0.0;
            } else if logical_key == Key::Named(NamedKey::Enter) {
                let token = self
                    .auth
                    .active()
                    .and_then(|a| self.auth.social_token(&a.uuid))
                    .map(str::to_string);
                let name = self.friends_prefs.add_buffer.trim().to_string();
                if let (Some(token), false) = (token, name.is_empty()) {
                    log::info!("friends: Enter → request '{}'", name);
                    self.social.submit_friend_request_by_name(&token, name);
                    self.friends_prefs.add_buffer.clear();
                    self.friends_prefs.add_focused = false;
                }
            } else if let Some(t) = text.as_ref() {
                for ch in t.chars() {
                    // MC names are ASCII alphanumeric + underscore,
                    // 1-16 chars, but accept broader and let the
                    // bot reject — keeps the input forgiving.
                    if !ch.is_control()
                        && self.friends_prefs.add_buffer.chars().count()
                            < NAME_MAX_LEN
                    {
                        self.friends_prefs.add_buffer.push(ch);
                        self.friends_prefs.add_focus_time = 0.0;
                    }
                }
            }
        } else if logical_key == Key::Named(NamedKey::Escape) && self.modal.open {
            log::info!("modal: Esc → closing");
            self.modal.close();
        } else if self.modal.open && self.modal.name_focused {
            // Name field text input — append printable chars, pop on
            // backspace. Only when the name field is the active
            // focus target (set by clicking the input).
            const NAME_MAX_LEN: usize = 48;
            if logical_key == Key::Named(NamedKey::Backspace) {
                self.modal.name.pop();
                self.modal.name_focus_time = 0.0; // restart caret blink
                if !self.modal.name.is_empty() {
                    self.modal.name_error = false;
                }
            } else if logical_key == Key::Named(NamedKey::Enter) {
                // Enter commits — same as clicking Create.
                if let Some(form) = try_submit_modal(&mut self.modal) {
                    commit_new_instance(
                        &mut self.instances,
                        &mut self.instance_prefs,
                        &self.versions,
                        &mut self.downloads,
                        form,
                        self.clock.elapsed,
                    );
                    self.modal.close();
                }
            } else if let Some(t) = text {
                for ch in t.chars() {
                    if !ch.is_control() && self.modal.name.chars().count() < NAME_MAX_LEN {
                        self.modal.name.push(ch);
                        self.modal.name_focus_time = 0.0;
                        self.modal.name_error = false;
                    }
                }
            }
        }
    }

    /// `WindowEvent::MouseWheel`.
    pub(crate) fn on_mouse_wheel(&mut self, window: &Arc<Window>, delta: MouseScrollDelta) {
        let scale = window.scale_factor();
        let card_pos = cursor_card_local(self.cursor, scale);
        let size = window.inner_size();
        let card_w = card_content_width(size, scale);
        let card_h = card_content_height(size, scale);

        // Convert delta to logical pixels. `LineDelta` lines are
        // platform-dependent — multiply by 32px to get a roughly
        // right wheel-tick-to-pixels mapping. `PixelDelta` is
        // already in physical pixels — divide by scale.
        let dy: f32 = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * 32.0,
            MouseScrollDelta::PixelDelta(p) => -(p.y as f32 / scale as f32),
        };

        // Routing priority:
        //  1. If a dropdown menu is open *anywhere*, route the
        //     wheel to it (so users can scroll the open list).
        //  2. If the modal is open, absorb the wheel even when no
        //     menu is open — prevents the underlying Instances
        //     panel from scrolling beneath a modal.
        //  3. Otherwise, scroll the active screen's primary
        //     scrollable region (currently only the Instances
        //     detail panel).
        let fonts = self.fonts.as_ref();
        let mut handled = false;

        // About modal absorbs wheel events outright — there's nothing
        // scrollable inside it, but we don't want the Instances panel
        // to scroll behind the dialog.
        if self.about_modal.open {
            let _ = dy;
            return;
        }

        if let Some(fonts) = fonts {
            // Modal dropdown takes precedence when modal is open.
            if self.modal.open {
                if let Some(slot) = self.modal.open_dropdown() {
                    // Extract just the count so the `Vec<&str>` borrow on
                    // `self.modal` ends before the `&mut VdropState`.
                    let opt_count = self.modal.dropdown_options(slot).map(|v| v.len());
                    if let Some(opt_count) = opt_count {
                        let layout = screens::new_instance_modal::compute_layout(
                            card_w, card_h, fonts,
                        );
                        let head = match slot {
                            ModalSlot::Version => layout.version_head,
                            ModalSlot::Loader => layout.loader_head,
                            _ => layout.version_head,
                        };
                        let (menu_bounds, _flip) =
                            ewo_render::widgets::menu_layout(head, opt_count, card_h);
                        if let Some(state) = self.modal.dropdown_state_mut(slot) {
                            state.scroll_by(dy, menu_bounds, opt_count);
                        }
                        handled = true;
                    }
                }
            } else {
                // Settings dropdown
                if !handled && self.screen == Screen::Settings {
                    if let Some(slot) = self.prefs.open_dropdown() {
                        if let Some(opts) = screens::settings::dropdown_options(slot) {
                            if let Some(head) = screens::settings::dropdown_head_for_slot(
                                slot, fonts, card_w, card_h,
                            ) {
                                let (menu_bounds, _flip) =
                                    ewo_render::widgets::menu_layout(
                                        head,
                                        opts.len(),
                                        card_h,
                                    );
                                if let Some(state) = self.prefs.dropdown_state_mut(slot) {
                                    state.scroll_by(dy, menu_bounds, opts.len());
                                }
                                handled = true;
                            }
                        }
                    }
                }
                // Instances dropdown
                if !handled && self.screen == Screen::Instances {
                    if let Some(slot) = self.instance_prefs.open_dropdown() {
                        if let Some(opts) = screens::instances::dropdown_options(slot) {
                            if let Some(head) = screens::instances::dropdown_head_for_slot(
                                slot,
                                fonts,
                                card_w,
                                card_h,
                                &self.instance_prefs,
                                &self.instances,
                            ) {
                                let (menu_bounds, _flip) =
                                    ewo_render::widgets::menu_layout(
                                        head,
                                        opts.len(),
                                        card_h,
                                    );
                                if let Some(state) =
                                    self.instance_prefs.dropdown_state_mut(slot)
                                {
                                    state.scroll_by(dy, menu_bounds, opts.len());
                                }
                                handled = true;
                            }
                        }
                    }
                }
            }
        }

        // Modal absorbs all remaining wheel events so the
        // background doesn't scroll beneath it.
        if !handled && self.modal.open {
            handled = true;
        }

        // Fall-through: Worlds list (left) or Instances detail
        // panel (right) scroll, depending on which side the cursor
        // is on.
        if !handled && self.screen == Screen::Instances {
            if let Some(fonts) = fonts {
                if card_pos.0 < 320.0 {
                    // Cursor is over the Worlds list column.
                    let max_scroll = screens::instances::list_max_scroll(
                        card_h,
                        fonts,
                        &self.instances,
                    );
                    self.instance_prefs.list_scroll =
                        (self.instance_prefs.list_scroll + dy).clamp(0.0, max_scroll);
                } else {
                    let panel =
                        screens::instances::detail_panel_bounds(card_w, card_h);
                    if rect_contains(&panel, card_pos) {
                        let max_scroll = screens::instances::detail_max_scroll(
                            card_w,
                            card_h,
                            fonts,
                            &self.instance_prefs,
                            &self.instances,
                        );
                        self.instance_prefs.detail_scroll = (self.instance_prefs.detail_scroll + dy)
                            .clamp(0.0, max_scroll);
                    }
                }
            }
        }

        // Settings — scroll the Keybinds / Modules tab list.
        if !handled && self.screen == Screen::Settings {
            if let Some(fonts) = fonts {
                let (content_h, visible_h) = match self.settings_tab {
                    SettingsTab::Keybinds => {
                        let l = screens::settings::keybinds_tab_layout(
                            fonts,
                            card_w,
                            card_h,
                            keybind::REGISTRY.len(),
                            self.prefs.settings_scroll,
                        );
                        (l.content_h, l.list_region.height())
                    }
                    SettingsTab::Modules => {
                        let l = screens::settings::modules_tab_layout(
                            fonts,
                            card_w,
                            card_h,
                            self.prefs.settings_scroll,
                        );
                        (l.content_h, l.list_region.height())
                    }
                    SettingsTab::PvpUtils => {
                        let l = screens::settings::pvp_tab_layout(
                            fonts,
                            card_w,
                            card_h,
                            self.prefs.settings_scroll,
                        );
                        (l.content_h, l.list_region.height())
                    }
                    _ => (0.0, 1.0),
                };
                let max = (content_h - visible_h).max(0.0);
                self.prefs.settings_scroll =
                    (self.prefs.settings_scroll + dy).clamp(0.0, max);
            }
        }
    }

    /// `WindowEvent::CursorMoved`.
    pub(crate) fn on_cursor_moved(&mut self, window: &Arc<Window>, position: PhysicalPosition<f64>) {
        self.cursor = position;
        let scale = window.scale_factor();
        let card_pos = cursor_card_local(position, scale);
        let size = window.inner_size();
        let card_w = card_content_width(size, scale);
        let card_h = card_content_height(size, scale);
        let time = self.clock.elapsed;

        // Top tab bar hover → glow (every screen). Suppressed while a
        // modal is open so tabs don't light up under the shroud.
        let any_modal = self.modal.open
            || self.about_modal.open
            || self.launcher_link_modal.open;
        self.hovered_tab = if any_modal {
            None
        } else {
            self.fonts.as_ref().and_then(|fonts| {
                screens::tab_bounds(card_w, fonts)
                    .into_iter()
                    .find(|(_, r)| rect_contains(r, card_pos))
                    .map(|(s, _)| s)
            })
        };
        // "‹ Main menu" back-link hover (for its glow).
        self.back_link_hover = !any_modal
            && matches!(self.screen, Screen::Settings | Screen::Instances)
            && rect_contains(
                &skia_safe::Rect::from_xywh(34.0, 38.0, 180.0, 42.0),
                card_pos,
            );
        // Top-right window buttons hover.
        let (min_btn, close_btn) = app_window::window_button_bounds(card_w);
        self.min_btn_hover = rect_contains(&min_btn, card_pos);
        self.close_btn_hover = rect_contains(&close_btn, card_pos);

        // When a modal is open it absorbs all hover state so the
        // background screen doesn't react under it. Otherwise drive
        // the active screen's widget + list/button hovers normally.
        if self.about_modal.open {
            // About modal absorbs hover; clear background state so
            // glows/highlights don't bleed through.
            self.instance_prefs.list_hover = None;
            self.instance_prefs.delete_hover = None;
            self.instance_prefs.rename_hover = false;
            self.instance_prefs.add_hover = false;
            self.instance_prefs.sort_hover = false;
        } else if self.modal.open {
            drive_modal_widgets(
                &mut self.modal,
                self.fonts.as_ref(),
                card_pos,
                self.mouse_down,
                card_w,
                card_h,
            );
            // Clear background hover state so the modal feels modal.
            self.instance_prefs.list_hover = None;
            self.instance_prefs.add_hover = false;
            self.launch_button = VbtnState::default();
            for s in self.menu_items.iter_mut() {
                *s = VbtnState::default();
            }
        } else if self.screen == Screen::Settings {
            let account_uuids = self.auth.account_uuids();
            let link_status = self.link_status_view();
            let changed = drive_settings_sliders(
                &mut self.prefs,
                self.settings_tab,
                self.fonts.as_ref(),
                card_pos,
                self.mouse_down,
                card_w,
                card_h,
                &account_uuids,
                &self.profiles,
                link_status,
            );
            if changed {
                profile::save(&self.prefs.to_config(), &self.settings);
                if let Some(b) = self.backend.as_ref() {
                    b.set_vsync(self.prefs.vsync.on);
                }
            }
            self.instance_prefs.list_hover = None;
            self.instance_prefs.add_hover = false;
        } else if self.screen == Screen::Instances {
            let changed = drive_instance_widgets(
                &mut self.instance_prefs,
                &self.instances,
                self.fonts.as_ref(),
                card_pos,
                self.mouse_down,
                card_w,
                card_h,
            );
            if changed {
                sync_instance_config(&mut self.instances, &self.instance_prefs);
                persistence::save_instances(&self.instances);
            }
            // List-row + "+" button + sort button + × delete hover
            // + ✎ rename hover.
            if let Some(fonts) = self.fonts.as_ref() {
                self.instance_prefs.rename_hover =
                    screens::instances::rename_button_bounds(
                        card_w, card_h, fonts, &self.instances, &self.instance_prefs,
                    )
                    .map(|r| rect_contains(&r, card_pos))
                    .unwrap_or(false);
                let mut hover: Option<usize> = None;
                let mut delete_hover: Option<usize> = None;
                for (i, rect) in screens::instances::list_row_bounds(
                    card_h,
                    fonts,
                    &self.instances,
                    &self.instance_prefs,
                )
                .iter()
                .enumerate()
                {
                    if rect_contains(rect, card_pos) {
                        hover = Some(i);
                    }
                    let del = screens::instances::delete_button_bounds(*rect);
                    if rect_contains(&del, card_pos) {
                        delete_hover = Some(i);
                    }
                }
                self.instance_prefs.list_hover = hover;
                self.instance_prefs.delete_hover = delete_hover;
                self.instance_prefs.add_hover = rect_contains(
                    &screens::instances::add_button_bounds(),
                    card_pos,
                );
                self.instance_prefs.sort_hover = rect_contains(
                    &screens::instances::sort_button_bounds(fonts),
                    card_pos,
                );
            }
        } else {
            // Clear lingering hover state when off the Instances screen.
            self.instance_prefs.list_hover = None;
            self.instance_prefs.add_hover = false;
        }
        if let Some(overlay) = self.dev_overlay.as_mut() {
            drive_dev_overlay(overlay, card_pos, self.mouse_down, card_w, card_h);
        }

        // Update launch-button + main-menu hover state. Modal-open
        // suppresses these so background buttons don't react under
        // a modal (new-instance, About, or launcher-link).
        let any_modal_open = self.modal.open
            || self.about_modal.open
            || self.launcher_link_modal.open;
        if any_modal_open {
            self.launch_button = VbtnState::default();
        } else if let Some(b) = self.launch_button_bounds(card_w) {
            self.launch_button.update(card_pos, b, self.mouse_down, time);
        } else {
            self.launch_button = VbtnState::default();
        }

        // Friends-screen buttons hover (link-launcher / add-friend) —
        // without this the buttons never animate and read as dead.
        if !any_modal_open && self.screen == Screen::Friends {
            if let Some(fonts) = self.fonts.as_ref() {
                let counts = match self.social.friends() {
                    social::FriendsListState::Loaded(list) => screens::FriendsCounts {
                        friends: list.friends.len(),
                        incoming: list.incoming.len(),
                        outgoing: list.outgoing.len(),
                    },
                    _ => screens::FriendsCounts::default(),
                };
                let layout = screens::friends_layout(card_w, fonts, counts);
                self.friends_prefs.link_launcher_btn.update(
                    card_pos,
                    layout.link_launcher_btn,
                    self.mouse_down,
                    time,
                );
                self.friends_prefs.add_submit_btn.update(
                    card_pos,
                    layout.add_submit,
                    self.mouse_down,
                    time,
                );
            }
        }

        let mut hovering_menu = false;
        if !any_modal_open && self.screen == Screen::MainMenu {
            if let Some(fonts) = self.fonts.as_ref() {
                let bounds = screens::main_menu::menu_item_bounds(card_w, card_h, fonts);
                for (i, b) in bounds.iter().enumerate() {
                    self.menu_items[i].update(card_pos, *b, self.mouse_down, time);
                    if self.menu_items[i].hover {
                        hovering_menu = true;
                    }
                }
                // Heading hover-glow — fires per-glyph stagger when
                // the cursor enters/exits the EwoClient title bbox.
                let heading = screens::main_menu::heading_bounds(fonts);
                let over_heading = rect_contains(&heading, card_pos);
                self.heading_hover.update(over_heading, time);
            }
            // H6 — network widget hover (lower-left card).
            let server_rect = screens::server_widget_bounds(card_w, card_h);
            self.server_widget_hover = rect_contains(&server_rect, card_pos);
        } else {
            for s in self.menu_items.iter_mut() {
                *s = VbtnState::default();
            }
            // Clear heading-hover when off the main menu so the
            // glow doesn't survive a screen change.
            self.heading_hover.update(false, time);
            self.server_widget_hover = false;
        }

        // About modal — drive the Close button hover so the ghost
        // glow tracks the cursor without needing a click.
        if self.about_modal.open {
            let close_rect =
                screens::about_modal::close_button_bounds(card_w, card_h);
            self.about_modal.close_btn.handle(card_pos, close_rect, false);
        }

        // Phase H2 launcher-link modal — drive Cancel + Submit hover.
        if self.launcher_link_modal.open {
            let cancel_rect = screens::launcher_link_modal::cancel_button_bounds(
                card_w, card_h,
            );
            let submit_rect = screens::launcher_link_modal::submit_button_bounds(
                card_w, card_h,
            );
            self.launcher_link_modal.cancel_btn.handle(
                card_pos, cancel_rect, false,
            );
            self.launcher_link_modal.submit_btn.update(
                card_pos,
                submit_rect,
                self.mouse_down,
                time,
            );
        }

        // Hover priority: tab bar → menu items → launch button → window zones.
        let hovering_tab = if let Some(fonts) = self.fonts.as_ref() {
            screens::tab_bounds(card_w, fonts)
                .iter()
                .any(|(_, r)| rect_contains(r, card_pos))
        } else {
            false
        };

        let hovering_settings_tab = if self.screen == Screen::Settings {
            if let Some(fonts) = self.fonts.as_ref() {
                let hit = screens::settings::sidebar_tab_bounds(fonts)
                    .into_iter()
                    .find(|(_, r)| rect_contains(r, card_pos))
                    .map(|(t, _)| t);
                self.prefs.hovered_sidebar_tab = hit;
                hit.is_some()
            } else {
                self.prefs.hovered_sidebar_tab = None;
                false
            }
        } else {
            self.prefs.hovered_sidebar_tab = None;
            false
        };

        let hovering_settings_widget = if self.screen == Screen::Settings {
            if let Some(fonts) = self.fonts.as_ref() {
                screens::settings::widget_bounds(
                    self.settings_tab, fonts, card_w, card_h,
                )
                .iter()
                .any(|(_, r)| rect_contains(r, card_pos))
            } else {
                false
            }
        } else {
            false
        };

        let hovering_instance_widget = if self.screen == Screen::Instances {
            if let Some(fonts) = self.fonts.as_ref() {
                let widgets = screens::instances::widget_bounds(
                    card_w, card_h, fonts, &self.instance_prefs, &self.instances,
                );
                let on_widget = widgets
                    .iter()
                    .any(|(_, r)| rect_contains(r, card_pos));
                let row_rects = screens::instances::list_row_bounds(
                    card_h, fonts, &self.instances, &self.instance_prefs,
                );
                let on_list_row =
                    row_rects.iter().any(|r| rect_contains(r, card_pos));
                let on_add = rect_contains(
                    &screens::instances::add_button_bounds(),
                    card_pos,
                );
                let on_sort = rect_contains(
                    &screens::instances::sort_button_bounds(fonts),
                    card_pos,
                );
                // × buttons are inside row rects, but we want the
                // pointer cursor regardless — `on_list_row` already
                // covers them.
                let on_rename = screens::instances::rename_button_bounds(
                    card_w, card_h, fonts, &self.instances, &self.instance_prefs,
                )
                .map(|r| rect_contains(&r, card_pos))
                .unwrap_or(false);
                on_widget || on_list_row || on_add || on_sort || on_rename
            } else {
                false
            }
        } else {
            false
        };

        // Modal hover: when the modal is open, the cursor flips
        // based on which control it's over. Name field → text
        // I-beam; buttons / dropdowns / slider → pointer; anywhere
        // else (including the shroud) → default arrow.
        let modal_cursor = if self.modal.open {
            if let Some(fonts) = self.fonts.as_ref() {
                let layout = screens::new_instance_modal::compute_layout(
                    card_w, card_h, fonts,
                );
                if rect_contains(&layout.name_input, card_pos) {
                    Some(CursorIcon::Text)
                } else {
                    let widgets = screens::new_instance_modal::widget_bounds(
                        card_w, card_h, fonts,
                    );
                    if widgets.iter().any(|(_, r)| rect_contains(r, card_pos)) {
                        Some(CursorIcon::Pointer)
                    } else {
                        Some(CursorIcon::Default)
                    }
                }
            } else {
                Some(CursorIcon::Default)
            }
        } else {
            None
        };

        if let Some(icon) = modal_cursor {
            window.set_cursor(icon);
        } else if hovering_tab
            || hovering_menu
            || hovering_settings_tab
            || hovering_settings_widget
            || hovering_instance_widget
            || self.launch_button.hover
        {
            window.set_cursor(CursorIcon::Pointer);
        } else {
            update_cursor_icon(&window, &self.cursor, size, scale);
        }
    }

    /// `WindowEvent::MouseInput` for the left button.
    pub(crate) fn on_left_mouse(&mut self, event_loop: &ActiveEventLoop, window: &Arc<Window>, state: ElementState) {
        let pressed = matches!(state, ElementState::Pressed);
        self.mouse_down = pressed;

        let scale = window.scale_factor();
        let card_pos = cursor_card_local(self.cursor, scale);
        let size = window.inner_size();
        let card_w = card_content_width(size, scale);
        let card_h = card_content_height(size, scale);
        let time = self.clock.elapsed;

        // Step -2: top-right window buttons (minimize / close) — handled
        // before anything else (incl. modals + dev overlay) so they
        // always work. They're excluded from the drag caption in
        // `hit_test`, so the press reaches here.
        if pressed {
            let (min_btn, close_btn) = app_window::window_button_bounds(card_w);
            if rect_contains(&close_btn, card_pos) {
                log::info!("window: close button → exit");
                event_loop.exit();
                return;
            }
            if rect_contains(&min_btn, card_pos) {
                log::info!("window: minimize button");
                if let Some(w) = self.window.as_ref() {
                    w.set_minimized(true);
                }
                return;
            }
        }

        // Step -1: dev overlay (when --dev) — sits above everything,
        // including the modal. Absorbs input when cursor is over it.
        if let Some(overlay) = self.dev_overlay.as_mut() {
            let panel = screens::dev_overlay::panel_bounds(card_w, card_h);
            drive_dev_overlay(overlay, card_pos, pressed, card_w, card_h);
            if rect_contains(&panel, card_pos) {
                if pressed {
                    let vsync_changed =
                        handle_dev_overlay_press(overlay, card_pos, card_w, card_h);
                    if vsync_changed {
                        if let Some(backend) = self.backend.as_ref() {
                            backend.set_vsync(overlay.vsync);
                        }
                    }
                }
                return;
            }
        }

        // Step 0a: About modal — when open, absorbs all input. Close
        // button click closes; shroud click closes; press anywhere
        // else inside the card is a no-op so the modal can't be
        // dismissed by misclicks on the card itself.
        if self.about_modal.open {
            let close_rect =
                screens::about_modal::close_button_bounds(card_w, card_h);
            let close_clicked =
                self.about_modal.close_btn.handle(card_pos, close_rect, pressed);
            if close_clicked {
                log::info!("about: Close clicked");
                self.about_modal.close();
            } else if pressed
                && screens::about_modal::shroud_consumes(card_pos, card_w, card_h)
            {
                log::info!("about: shroud click → closing");
                self.about_modal.close();
            }
            return;
        }

        // Step 0b (Phase H2): launcher-link modal absorbs all input.
        // Cancel closes + clears redeem; Submit fires submit_link_code
        // when the code is ready; shroud click closes.
        if self.launcher_link_modal.open {
            let cancel_rect = screens::launcher_link_modal::cancel_button_bounds(
                card_w, card_h,
            );
            let submit_rect = screens::launcher_link_modal::submit_button_bounds(
                card_w, card_h,
            );
            let cancel_clicked = self.launcher_link_modal.cancel_btn.handle(
                card_pos, cancel_rect, pressed,
            );
            let submit_clicked = self.launcher_link_modal.submit_btn.update(
                card_pos,
                submit_rect,
                self.mouse_down,
                time,
            );
            if cancel_clicked {
                log::info!("launcher-link modal: Cancel clicked");
                self.launcher_link_modal.close();
                self.social.clear_link_redeem();
            } else if submit_clicked && self.launcher_link_modal.is_ready() {
                let code = self.launcher_link_modal.code.clone();
                log::info!("launcher-link modal: Submit clicked");
                self.social.submit_link_code(code);
            } else if pressed
                && screens::launcher_link_modal::shroud_consumes(
                    card_pos, card_w, card_h,
                )
            {
                log::info!("launcher-link modal: shroud click → closing");
                self.launcher_link_modal.close();
                self.social.clear_link_redeem();
            }
            return;
        }

        // Step 0: modal — when open, the modal absorbs all input.
        // `drive_modal_widgets` runs first so slider drags start on
        // the rising edge inside bounds; `handle_modal_press` runs
        // after to consume button / dropdown / shroud clicks.
        if self.modal.open {
            drive_modal_widgets(
                &mut self.modal,
                self.fonts.as_ref(),
                card_pos,
                pressed,
                card_w,
                card_h,
            );
            if pressed {
                handle_modal_press(
                    &mut self.modal,
                    &mut self.instances,
                    &mut self.instance_prefs,
                    &self.versions,
                    &mut self.downloads,
                    self.fonts.as_ref(),
                    card_pos,
                    card_w,
                    card_h,
                    self.clock.elapsed,
                );
            }
            return;
        }

        // Step 1: tab bar hit-test (priority over everything else).
        let mut handled = false;
        if pressed {
            if let Some(fonts) = self.fonts.as_ref() {
                for (target, rect) in screens::tab_bounds(card_w, fonts) {
                    if rect_contains(&rect, card_pos) {
                        if self.screen != target {
                            log::info!("nav: {:?} → {:?}", self.screen, target);
                            self.screen = target;
                            self.launch_button = VbtnState::default();
                            for s in self.menu_items.iter_mut() {
                                *s = VbtnState::default();
                            }
                            self.prefs.close_dropdowns();
                            self.instance_prefs.close_dropdowns();
                            self.keybind_capture = None;
                            self.prefs.keybind_request = None;
                            self.prefs.profile_renaming = None;
                            self.prefs.profile_rename_buffer.clear();
                            self.modal.close();
                            // Trigger the tab fade-in when arriving at
                            // Settings, so the active tab's content
                            // greets the user with the same animation
                            // it plays when they switch tabs.
                            if target == Screen::Settings {
                                self.prefs.tab_changed_at = Some(time);
                                self.prefs.settings_scroll = 0.0;
                            }
                            // Demo affordance: clicking the LAUNCHING
                            // tab without an active launch kicks off
                            // a fresh synthetic one so the screen is
                            // never empty.
                            if target == Screen::Launching
                                && self.launching.start_time.is_none()
                            {
                                let (inst_name, inst_meta) = self
                                    .instances
                                    .get(self.instance_prefs.selected)
                                    .map(|i| {
                                        (
                                            i.name.clone(),
                                            format!(
                                                "{} · ADOPTIUM 21 · {} GB",
                                                i.version,
                                                self.instance_prefs.ram.value as i32,
                                            ),
                                        )
                                    })
                                    .unwrap_or_else(|| {
                                        (
                                            "Velvet Hours".to_string(),
                                            "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                                        )
                                    });
                                self.launching.enter(time, &inst_name, &inst_meta);
                            }
                        }
                        handled = true;
                        break;
                    }
                }
            }
        }

        // Step 1.5: "‹ Main menu" back-link (top-left of the
        // screen-head screens) — navigate home.
        if !handled
            && pressed
            && matches!(self.screen, Screen::Settings | Screen::Instances)
        {
            let back_rect = skia_safe::Rect::from_xywh(34.0, 38.0, 180.0, 42.0);
            if rect_contains(&back_rect, card_pos) {
                log::info!("nav: {:?} → MainMenu (back-link)", self.screen);
                self.screen = Screen::MainMenu;
                self.prefs.close_dropdowns();
                self.instance_prefs.close_dropdowns();
                handled = true;
            }
        }

        // Step 2: main-menu sidebar items.
        if !handled && self.screen == Screen::MainMenu {
            if let Some(fonts) = self.fonts.as_ref() {
                let bounds = screens::main_menu::menu_item_bounds(card_w, card_h, fonts);
                for (i, b) in bounds.iter().enumerate() {
                    let clicked =
                        self.menu_items[i].update(card_pos, *b, pressed, time);
                    if clicked {
                        match MAIN_MENU_ACTIONS[i] {
                            MenuAction::Navigate(target) => {
                                log::info!("nav (menu): {:?} → {:?}", self.screen, target);
                                self.screen = target;
                            }
                            MenuAction::About => {
                                log::info!("about: clicked → opening About modal");
                                self.about_modal.open();
                            }
                            MenuAction::Quit => {
                                log::info!("quit: closing app");
                                event_loop.exit();
                            }
                        }
                    }
                    if self.menu_items[i].hover && pressed {
                        handled = true;
                    }
                }
            }
            // H6 — click the network widget to join the chickenedin
            // lobby (only when the network reports online).
            if !handled {
                let server_rect = screens::server_widget_bounds(card_w, card_h);
                if rect_contains(&server_rect, card_pos) {
                    let online = matches!(
                        self.social.server_status(),
                        Some(s) if s.online
                    );
                    if online {
                        log::info!("h6: network widget clicked → joining lobby");
                        self.start_launch(
                            self.instance_prefs.selected,
                            Some(social::CHICKENEDIN_LOBBY_ADDR.to_string()),
                            time,
                        );
                    }
                    handled = true;
                }
            }
        }

        // Step 2.5: settings sidebar tab switch.
        if !handled && pressed && self.screen == Screen::Settings {
            if let Some(fonts) = self.fonts.as_ref() {
                for (tab, rect) in screens::settings::sidebar_tab_bounds(fonts) {
                    if rect_contains(&rect, card_pos) {
                        if self.settings_tab != tab {
                            log::info!(
                                "settings: {:?} → {:?}",
                                self.settings_tab, tab
                            );
                            self.settings_tab = tab;
                            self.prefs.close_dropdowns();
                            // A tab switch abandons a pending keybind
                            // capture or an in-progress profile rename.
                            self.keybind_capture = None;
                            self.prefs.keybind_request = None;
                            self.prefs.profile_renaming = None;
                            self.prefs.profile_rename_buffer.clear();
                            self.prefs.tab_changed_at =
                                Some(self.clock.elapsed);
                            self.prefs.settings_scroll = 0.0;
                        }
                        handled = true;
                        break;
                    }
                }
            }
        }

        // Step 2.7: settings widget interaction. Toggles flip on the
        // press edge; sliders begin a drag (drag continues in
        // CursorMoved via `drive_settings_sliders`); dropdown heads
        // toggle the menu open/closed; clicks on open menu rows
        // commit the selection; clicks elsewhere close the menu.
        // Phase H5 — Friends screen press handling. Inline because
        // the surface is small (5 click targets + N row buttons).
        if !handled && pressed && self.screen == Screen::Friends {
            if let Some(fonts) = self.fonts.as_ref() {
                let counts = if let social::FriendsListState::Loaded(list) =
                    self.social.friends()
                {
                    screens::FriendsCounts {
                        friends: list.friends.len(),
                        incoming: list.incoming.len(),
                        outgoing: list.outgoing.len(),
                    }
                } else {
                    screens::FriendsCounts::default()
                };
                let layout = screens::friends_layout(card_w, fonts, counts);
                let active_token: Option<String> = self
                    .auth
                    .active()
                    .and_then(|a| self.auth.social_token(&a.uuid))
                    .map(str::to_string);

                if active_token.is_none() {
                    // NotLinked: clicking the centered button opens
                    // the launcher-link modal (same path as the
                    // rose line in Settings → Account).
                    if rect_contains(&layout.link_launcher_btn, card_pos) {
                        log::info!("friends: link button → open launcher-link modal");
                        self.social.clear_link_redeem();
                        self.launcher_link_modal.open();
                        handled = true;
                    }
                } else if let Some(token) = active_token.as_deref() {
                    // Loaded path. add input → focus.
                    if rect_contains(&layout.add_input, card_pos) {
                        self.friends_prefs.add_focused = true;
                        self.friends_prefs.add_focus_time = 0.0;
                        handled = true;
                    } else if rect_contains(&layout.add_submit, card_pos) {
                        let name = self.friends_prefs.add_buffer.trim().to_string();
                        if !name.is_empty() {
                            log::info!("friends: + Add → request '{}'", name);
                            self.social.submit_friend_request_by_name(token, name);
                            self.friends_prefs.add_buffer.clear();
                            self.friends_prefs.add_focused = false;
                        }
                        handled = true;
                    } else {
                        // Click landed away from the input; lose focus.
                        self.friends_prefs.add_focused = false;
                        // Walk row buttons for accept/decline/remove.
                        for btn in &layout.row_buttons {
                            if !rect_contains(&btn.rect, card_pos) {
                                continue;
                            }
                            // H6 — "Join" launches into the friend's
                            // current server (presence.server_addr),
                            // not a friend-graph mutation.
                            if btn.kind == screens::RowButtonKind::Join {
                                let addr = if let social::FriendsListState::Loaded(list) =
                                    self.social.friends()
                                {
                                    list.friends
                                        .get(btn.index)
                                        .and_then(|e| e.presence.as_ref())
                                        .and_then(|p| p.server_addr.clone())
                                } else {
                                    None
                                };
                                if let Some(addr) = addr {
                                    log::info!(
                                        "friends: join row {} → {}",
                                        btn.index, addr
                                    );
                                    self.start_launch(
                                        self.instance_prefs.selected,
                                        Some(addr),
                                        time,
                                    );
                                }
                                handled = true;
                                break;
                            }
                            let target_id = if let social::FriendsListState::Loaded(list) =
                                self.social.friends()
                            {
                                match btn.kind {
                                    screens::RowButtonKind::Accept
                                    | screens::RowButtonKind::Decline => list
                                        .incoming
                                        .get(btn.index)
                                        .map(|e| e.discord_id.clone()),
                                    screens::RowButtonKind::Remove => list
                                        .friends
                                        .get(btn.index)
                                        .map(|e| e.discord_id.clone()),
                                    // Handled above with an early break.
                                    screens::RowButtonKind::Join => None,
                                }
                            } else {
                                None
                            };
                            if let Some(other) = target_id {
                                match btn.kind {
                                    screens::RowButtonKind::Accept => {
                                        log::info!("friends: accept {}", other);
                                        self.social.respond_friend_request(
                                            token, other, true,
                                        );
                                    }
                                    screens::RowButtonKind::Decline => {
                                        log::info!("friends: decline {}", other);
                                        self.social.respond_friend_request(
                                            token, other, false,
                                        );
                                    }
                                    screens::RowButtonKind::Remove => {
                                        log::info!("friends: remove {}", other);
                                        self.social.remove_friend(token, other);
                                    }
                                    // Join is handled above (early break).
                                    screens::RowButtonKind::Join => {}
                                }
                                handled = true;
                            }
                            break;
                        }
                    }
                }
            }
        }

        if self.screen == Screen::Settings {
            let account_uuids = self.auth.account_uuids();
            let link_status = self.link_status_view();
            let mut changed = drive_settings_sliders(
                &mut self.prefs,
                self.settings_tab,
                self.fonts.as_ref(),
                card_pos,
                pressed,
                card_w,
                card_h,
                &account_uuids,
                &self.profiles,
                link_status,
            );
            if !handled && pressed {
                let (h, c) = handle_settings_press(
                    &mut self.prefs,
                    self.settings_tab,
                    self.fonts.as_ref(),
                    card_pos,
                    card_w,
                    card_h,
                    &account_uuids,
                    &self.profiles,
                    link_status,
                );
                handled = h;
                changed = changed || c;
            }
            if changed {
                profile::save(&self.prefs.to_config(), &self.settings);
                if let Some(b) = self.backend.as_ref() {
                    b.set_vsync(self.prefs.vsync.on);
                }
            }
        }

        // Step 2.75: instances list "+" button — opens the
        // new-instance modal.
        if !handled && pressed && self.screen == Screen::Instances {
            let plus_rect = screens::instances::add_button_bounds();
            if rect_contains(&plus_rect, card_pos) {
                log::info!("instances: + clicked → opening new-instance modal");
                self.modal.open();
                handled = true;
            }
        }

        // Step 2.754: ✎ rename icon — enters rename mode for
        // the currently-selected instance.
        if !handled && pressed && self.screen == Screen::Instances {
            if let Some(fonts) = self.fonts.as_ref() {
                if let Some(r) = screens::instances::rename_button_bounds(
                    card_w,
                    card_h,
                    fonts,
                    &self.instances,
                    &self.instance_prefs,
                ) {
                    if rect_contains(&r, card_pos) {
                        if let Some(inst) = self
                            .instances
                            .get(self.instance_prefs.selected)
                        {
                            self.instance_prefs.renaming = true;
                            self.instance_prefs.rename_buffer = inst.name.clone();
                            self.instance_prefs.rename_focus_time = 0.0;
                            log::info!("rename: editing \"{}\"", inst.name);
                        }
                        handled = true;
                    }
                }
            }
        }

        // Step 2.755: × delete button — must run before
        // click-to-select since × sits inside the row's hit-rect.
        if !handled && pressed && self.screen == Screen::Instances {
            if let Some(fonts) = self.fonts.as_ref() {
                let order = screens::instances::display_order(
                    &self.instances,
                    self.instance_prefs.sort_mode,
                );
                let row_rects = screens::instances::list_row_bounds(
                    card_h, fonts, &self.instances, &self.instance_prefs,
                );
                for (display_idx, row_rect) in row_rects.iter().enumerate() {
                    let del_rect = screens::instances::delete_button_bounds(*row_rect);
                    if rect_contains(&del_rect, card_pos) {
                        let underlying = order[display_idx];
                        delete_instance(
                            &mut self.instances,
                            &mut self.instance_prefs,
                            underlying,
                            self.clock.elapsed,
                        );
                        handled = true;
                        break;
                    }
                }
            }
        }

        // Step 2.76: instance list rows — click-to-select. Click
        // dispatch uses display order (visual position), then maps
        // back to the underlying index via `display_order`.
        if !handled && pressed && self.screen == Screen::Instances {
            if let Some(fonts) = self.fonts.as_ref() {
                let order = screens::instances::display_order(
                    &self.instances,
                    self.instance_prefs.sort_mode,
                );
                for (display_idx, rect) in screens::instances::list_row_bounds(
                    card_h, fonts, &self.instances, &self.instance_prefs,
                )
                .iter()
                .enumerate()
                {
                    if rect_contains(rect, card_pos) {
                        let underlying = order[display_idx];
                        if underlying != self.instance_prefs.selected {
                            log::info!(
                                "instances: select {} → {}",
                                self.instance_prefs.selected, underlying
                            );
                            self.instance_prefs.select(&self.instances, underlying);
                            self.instance_prefs.selected_at =
                                Some(self.clock.elapsed);
                        }
                        handled = true;
                        break;
                    }
                }
            }
        }

        // Step 2.77: sort label cycle.
        if !handled && pressed && self.screen == Screen::Instances {
            if let Some(fonts) = self.fonts.as_ref() {
                let r = screens::instances::sort_button_bounds(fonts);
                if rect_contains(&r, card_pos) {
                    self.instance_prefs.sort_mode =
                        self.instance_prefs.sort_mode.cycle();
                    log::info!(
                        "instances: sort → {}",
                        self.instance_prefs.sort_mode.label()
                    );
                    handled = true;
                }
            }
        }

        // Step 2.8: instances detail widget interaction. Sliders for
        // RAM / render distance, plus the Java runtime dropdown.
        if self.screen == Screen::Instances {
            let changed = drive_instance_widgets(
                &mut self.instance_prefs,
                &self.instances,
                self.fonts.as_ref(),
                card_pos,
                pressed,
                card_w,
                card_h,
            );
            if changed {
                sync_instance_config(&mut self.instances, &self.instance_prefs);
                persistence::save_instances(&self.instances);
            }
            if !handled && pressed {
                handled = handle_instances_press(
                    &mut self.instance_prefs,
                    &mut self.instances,
                    self.fonts.as_ref(),
                    card_pos,
                    card_w,
                    card_h,
                );
            }
        }

        // Step 2.9: Launching screen's Retry/Back buttons. Only
        // active when the JVM has exited non-zero. Retry rebuilds
        // the LaunchPlan and respawns; Back returns to Instances.
        if !handled
            && pressed
            && self.screen == Screen::Launching
            && self.launching.ended_in_error()
        {
            let retry_rect =
                screens::launching::retry_button_bounds(card_w, card_h);
            let back_rect =
                screens::launching::cancel_button_bounds(card_w, card_h);
            if rect_contains(&retry_rect, card_pos) {
                log::info!("launching: Retry clicked");
                let inst_name = self.launching.instance_name.clone();
                let inst_meta = self.launching.instance_meta.clone();
                let ok = self.launch_slot_busy(time) || {
                    self.launching.reset_for_retry();
                    self.try_real_launch(
                        self.instance_prefs.selected,
                        &inst_name,
                        &inst_meta,
                        time,
                        PrepareFail::Error("retry could not start launch"),
                    )
                };
                if !ok {
                    // try_real_launch can return false silently
                    // when something's missing — surface that to
                    // the user as an error rather than a blank
                    // screen.
                    self.launching.push_real_line(
                        screens::RealSeverity::Warn,
                        "[ewo] retry could not start launch — see logs above".into(),
                        time,
                    );
                    self.launching.set_real_exit(Some(127), time);
                }
                handled = true;
            } else if rect_contains(&back_rect, card_pos) {
                log::info!("launching: Back clicked");
                self.launching.exit();
                self.screen = Screen::Instances;
                handled = true;
            }
        }

        // Step 3: active screen's launch button.
        if !handled {
            if let Some(b) = self.launch_button_bounds(card_w) {
                let clicked = self.launch_button.update(card_pos, b, pressed, time);
                if clicked {
                    // start_launch gates a Pending/missing instance
                    // internally; a plain Launch passes no server.
                    self.start_launch(self.instance_prefs.selected, None, time);
                }
                if self.launch_button.hover && pressed {
                    handled = true;
                }
            }
        }

        // Step 4: window drag/resize fallback.
        if pressed && !handled {
            let zone = hit_test(self.cursor, size, scale);
            match zone {
                Some(Zone::Caption) => {
                    let _ = window.drag_window();
                }
                Some(Zone::Resize(dir)) => {
                    let _ = window.drag_resize_window(dir);
                }
                None => {
                    if let Some(b) = self.backdrop.as_mut() {
                        b.disturb();
                    }
                }
            }
        }
    }

    /// `WindowEvent::RedrawRequested`: render one frame.
    pub(crate) fn on_redraw(&mut self, window: &Arc<Window>) {
        // Skip render + swap entirely when the window can't be
        // shown. On Windows, `wglSwapBuffers` on an obscured /
        // minimised window queues presentations in the GL driver
        // indefinitely (the compositor can't display them, so
        // they pile up) — that's the per-frame ~6 KB / frame
        // C++-side leak we hunted.
        //
        // Four signals, any of which skips this frame:
        //  - `Occluded(true)` — winit's official signal. Reliable
        //    on most platforms but on Win11 it sometimes fails to
        //    fire when another app takes fullscreen.
        //  - `is_minimized() == Some(true)` — minimised to taskbar.
        //  - `Win32 GetForegroundWindow() != our hwnd` — we're not
        //    the user's active app. The Win32 fallback that fires
        //    when winit's Focused / Occluded didn't.
        //  - (`!self.focused` is intentionally NOT a skip signal —
        //    the user might have a chat window focused while
        //    watching the launcher animations in another monitor.
        //    Foreground covers the actually-leak-causing case.)
        let minimized = window.is_minimized().unwrap_or(false);
        let foreground = window::is_foreground(&window);
        if self.occluded || minimized || !foreground {
            return;
        }
        self.clock.tick();
        let time = self.clock.elapsed;
        let dt = self.clock.dt;
        let screen = self.screen;

        // Smooth screen entrance: stamp the wall-time the screen
        // changed so the renderer can fade + slide the main menu in.
        if screen != self.prev_screen {
            self.prev_screen = screen;
            self.screen_enter_at = time;
        }

        // Auto-end celebrate after the configured duration.
        if let Some(end) = self.celebrate_until {
            if time >= end {
                if let Some(b) = self.backdrop.as_mut() {
                    b.celebrate(false);
                }
                self.celebrate_until = None;
            }
        }

        if let Some(backdrop) = self.backdrop.as_mut() {
            backdrop.update(dt);
        }

        // Tick widget hover animations.
        self.launch_button.tick(dt);
        for s in self.menu_items.iter_mut() {
            s.tick(dt);
        }
        self.prefs.tick(dt);
        self.instance_prefs.tick(dt);
        if self.instance_prefs.renaming {
            self.instance_prefs.rename_focus_time += dt;
        }
        if self.prefs.profile_renaming.is_some() {
            self.prefs.profile_rename_focus_time += dt;
        }
        self.modal.tick(dt);
        self.about_modal.tick(dt);
        self.launcher_link_modal.tick(dt);
        self.friends_prefs.tick(dt);
        self.auth.poll();
        // Social: drain probe results, then make sure every
        // signed-in account has a probe in flight or done.
        // `ensure_probed` is idempotent — already-probed UUIDs
        // are a no-op.
        self.social.poll();
        // Only accounts that are actually signed in (a live token)
        // are probed — none when signed out or offline.
        let signed_in: Vec<String> = self
            .auth
            .accounts()
            .iter()
            .filter(|a| !a.minecraft_token.is_empty())
            .map(|a| a.uuid.clone())
            .collect();
        for uuid in &signed_in {
            self.social.ensure_probed(uuid);
        }
        // FRIENDS overlay tab — when the friends list changes,
        // rewrite the per-profile `ewo-friends.txt` snapshot the
        // in-game cdylib reads. One tab-separated line per accepted
        // friend: <online 0|1>\t<name>\t<presence>\t<server_addr>.
        if self.social.take_friends_dirty() {
            if let social::FriendsListState::Loaded(list) = self.social.friends() {
                use std::fmt::Write as _;
                let mut s = String::new();
                for e in &list.friends {
                    let v = friend_entry_to_view(e);
                    let _ = writeln!(
                        s,
                        "{}\t{}\t{}\t{}",
                        if v.online { 1 } else { 0 },
                        v.display_name,
                        v.presence,
                        v.server_addr.unwrap_or_default(),
                    );
                }
                if let Some(dir) = profile::active_dir() {
                    if let Err(e) =
                        util::atomic_write(&dir.join("ewo-friends.txt"), s.as_bytes())
                    {
                        log::warn!("social: write ewo-friends.txt failed: {}", e);
                    }
                }
            }
        }
        // Phase H3: fire a presence heartbeat (rate-gated to 30s
        // inside `maybe_send_heartbeat`). Only when both an
        // active account AND a live social_token are available
        // — i.e. the user has completed the launcher-link flow.
        let heartbeat_inputs: Option<(String, String)> = self
            .auth
            .active()
            .and_then(|a| {
                self.auth
                    .social_token(&a.uuid)
                    .map(|t| (a.uuid.clone(), t.to_string()))
            });
        if let Some((mc_uuid, token)) = heartbeat_inputs {
            // H6: while a server-join JVM is alive, advertise
            // `in_game · <addr>` so friends see (and can join) us.
            // `launch_rx` is `Some` only while the child runs; once
            // it exits we fall back to the current launcher screen.
            let location = match (self.launch_rx.is_some(), self.active_server.as_deref())
            {
                (true, Some(addr)) => {
                    social::HeartbeatLocation::InGame { server_addr: addr }
                }
                _ => social::HeartbeatLocation::InLauncher {
                    screen: screen_name(self.screen),
                },
            };
            self.social.maybe_send_heartbeat(time, &mc_uuid, &token, location);
            // Phase H5: refresh friends list on the 30s cadence.
            // Post-mutation refreshes are chained inside the
            // social worker thread (see `dispatch_friend_action`).
            self.social.maybe_refresh_friends(time, &token);
        }
        // Phase H2: harvest a successful launcher-link redemption
        // immediately — persist the token next to the active MC
        // account, clear the redemption state, close the modal.
        let pending_token: Option<(String, String)> = if let
            social::LinkRedeemStatus::Success { token, .. } =
            self.social.link_redeem()
        {
            let token = token.clone();
            self.auth.active().map(|a| (a.uuid.clone(), token))
        } else {
            None
        };
        if let Some((uuid, token)) = pending_token {
            self.auth.set_social_token(&uuid, token);
            self.social.clear_link_redeem();
            self.launcher_link_modal.close();
        }
        // H6 — poll the public network status only while it's on
        // screen (the main-menu widget). 15s cadence inside the call.
        // Offline-first: a launcher with no signed-in account makes
        // no social / server-status calls at all.
        if self.screen == Screen::MainMenu && !signed_in.is_empty() {
            self.social.maybe_refresh_server_status(time);
        }
        self.versions.poll();
        self.downloads.poll();

        // Sync per-instance download progress for the list-row
        // badge. Only Pending instances are interesting; we
        // clear the map first so completed downloads stop
        // showing a stale percentage.
        self.instance_prefs.download_pct.clear();
        for inst in self.instances.iter() {
            if inst.status
                != ewo_render::screens::instances::InstanceStatus::Pending
            {
                continue;
            }
            let v = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
            if let Some(status) = self.downloads.status(v) {
                if let Some(total) = status.total {
                    if total > 0 {
                        let pct = ((status.downloaded as f64 / total as f64) * 100.0)
                            .clamp(0.0, 99.0)
                            as u32;
                        self.instance_prefs
                            .download_pct
                            .insert(inst.name.clone(), pct);
                    }
                }
            }
        }

        // Finish a JVM launch whose background preparation is done.
        self.poll_prepare(time);

        // Drain runtime (bundled-JRE) events. Surface progress
        // as Info lines on the launching screen; on Done, kick
        // the JRE-detector cache + retry the pending launch.
        let runtime_events = self.runtime.poll();
        for ev in runtime_events {
            match ev {
                runtime::RuntimeEvent::Resolved { major, info } => {
                    self.launching.push_real_line(
                        screens::RealSeverity::Info,
                        format!(
                            "[ewo] resolved Java {} → {} ({:.1} MB)",
                            major,
                            info.release_name,
                            info.size as f32 / 1_048_576.0
                        ),
                        time,
                    );
                }
                runtime::RuntimeEvent::Progress { downloaded, total } => {
                    // Drive the real pbar override (visible
                    // immediately as a smooth fill), and log a
                    // line every 10% step so the user sees
                    // discrete checkpoints in the log panel too.
                    if total > 0 {
                        let frac = downloaded as f32 / total as f32;
                        self.launching.set_real_progress(Some(frac));
                        let pct = frac * 100.0;
                        let bucket = (pct as u32) / 10 * 10;
                        if bucket > 0 && bucket % 10 == 0 {
                            let line = format!(
                                "[ewo] downloading runtime: {:>3}% ({} / {} MB)",
                                bucket,
                                downloaded / 1_048_576,
                                total / 1_048_576,
                            );
                            self.launching.push_real_line(
                                screens::RealSeverity::Info,
                                line,
                                time,
                            );
                        }
                    }
                }
                runtime::RuntimeEvent::Done { major, jre_dir } => {
                    // Clear the pbar override — synthetic curve
                    // takes back over while the JVM boots.
                    self.launching.set_real_progress(None);
                    self.launching.push_real_line(
                        screens::RealSeverity::Info,
                        format!(
                            "[ewo] Java {} extracted to {} — retrying launch…",
                            major,
                            jre_dir.display()
                        ),
                        time,
                    );
                    launch::jre::invalidate_cache();
                    // If the launch we deferred is for this
                    // major, retry it now.
                    if let Some(p) = self.pending_relaunch.clone() {
                        if p.waiting_for_major == major {
                            self.pending_relaunch = None;
                            let ok = self.try_real_launch(
                                p.instance_idx,
                                &p.instance_name,
                                &p.instance_meta,
                                time,
                                PrepareFail::Error("retry failed after JRE install"),
                            );
                            if !ok {
                                self.launching.push_real_line(
                                    screens::RealSeverity::Warn,
                                    "[ewo] retry failed after JRE install".into(),
                                    time,
                                );
                                self.launching.set_real_exit(Some(127), time);
                            }
                        }
                    }
                }
                runtime::RuntimeEvent::Failed { major, message } => {
                    log::warn!("runtime: Java {} fetch failed: {}", major, message);
                    self.launching.set_real_progress(None);
                    self.launching.push_real_line(
                        screens::RealSeverity::Warn,
                        format!("[ewo] Java {} fetch failed: {}", major, message),
                        time,
                    );
                    self.launching.set_real_exit(Some(127), time);
                    self.pending_relaunch = None;
                }
            }
        }

        // Drain JVM launch events into the launching screen.
        // Each line, every stage transition, and the exit code
        // arrives via this channel. When the JVM exits we drop
        // the receiver — the next launch will create a fresh one.
        let mut launch_finished = false;
        if let Some(rx) = self.launch_rx.as_ref() {
            while let Ok(event) = rx.try_recv() {
                match event {
                    launch::LaunchEvent::Started { pid, created } => {
                        log::info!("launch: JVM started (pid {pid})");
                        // Record PID + creation time so a zombie JVM
                        // (deadlocked in native teardown on exit) can
                        // be reaped before the next launch — and only
                        // that exact process. See launch::reaper.
                        self.active_launch_started_at = time;
                        self.active_launch_window_seen = false;
                        self.active_launch = created.map(|created| {
                            let t = launch::reaper::Tracked { pid, created };
                            launch::reaper::record(t);
                            t
                        });
                        // Don't minimize yet — the JVM has only just
                        // spawned; Minecraft's window is ~10-30s away.
                        // Arm a fallback deadline instead. We minimize
                        // the moment the game's window-ready marker shows
                        // in the log (see the `Line` arm), or when this
                        // deadline fires, whichever comes first. Restored
                        // on JVM exit (the `launch_finished` block below).
                        self.pending_minimize = Some(time + MINIMIZE_FALLBACK_SECS);
                    }
                    launch::LaunchEvent::Line { severity, text } => {
                        if is_window_ready_marker(&text) {
                            self.active_launch_window_seen = true;
                        }
                        // Minimize as soon as the game is visibly coming
                        // up. These markers are logged right as the client
                        // creates its render backend / window — the point
                        // where MC becomes visible — so this tracks "the
                        // game is on screen now" far better than the spawn
                        // event does. Only acts while a minimize is armed.
                        if self.pending_minimize.is_some()
                            && is_window_ready_marker(&text)
                        {
                            if let Some(win) = self.window.as_ref() {
                                win.set_minimized(true);
                            }
                            self.pending_minimize = None;
                            log::info!("launch: game window up — minimized launcher");
                        }
                        let sev = match severity {
                            launch::Severity::Info => screens::RealSeverity::Info,
                            launch::Severity::Warn => screens::RealSeverity::Warn,
                        };
                        self.launching.push_real_line(sev, text, time);
                    }
                    launch::LaunchEvent::Exited(code) => {
                        log::info!("launch: JVM exited code={:?}", code);
                        self.launching.set_real_exit(code, time);
                        // Dump the in-memory log to disk so the
                        // user can grab it later (especially on
                        // a crash). Best-effort: errors don't
                        // surface to the UI.
                        persist_launch_log(
                            self.active_launch_instance_id.as_deref().unwrap_or(""),
                            &self.launching.instance_name,
                            self.launching
                                .real_log
                                .as_deref()
                                .unwrap_or(&[]),
                            code,
                        );
                        launch_finished = true;
                    }
                    launch::LaunchEvent::SpawnFailed(msg) => {
                        log::warn!("launch: spawn failed: {}", msg);
                        self.launching.push_real_line(
                            screens::RealSeverity::Warn,
                            format!("[ewo] spawn failed: {}", msg),
                            time,
                        );
                        self.launching.set_real_exit(Some(127), time);
                        launch_finished = true;
                    }
                }
            }
        }
        // Fallback minimize: if the game has been starting for a while
        // but we never saw a window-ready marker (modded logs vary),
        // minimize once the armed deadline passes so the hand-off still
        // happens.
        if let Some(deadline) = self.pending_minimize {
            if time >= deadline {
                if let Some(win) = self.window.as_ref() {
                    win.set_minimized(true);
                }
                self.pending_minimize = None;
                log::info!("launch: minimize fallback fired — minimized launcher");
            }
        }

        if launch_finished {
            self.launch_rx = None;
            // JVM reported exit — forget its record (see launch::reaper).
            if let Some(t) = self.active_launch.take() {
                launch::reaper::forget(&t);
            }
            self.active_launch_instance_id = None;
            // A launch that ends before it ever minimized (fast crash,
            // spawn failure): disarm so we don't minimize after the fact.
            self.pending_minimize = None;
            // The game (or a failed spawn) is done — bring the launcher
            // back from the taskbar and re-focus it so the user lands on
            // the post-launch screen. `set_minimized(false)` is a no-op
            // if we never minimized (e.g. a spawn that failed before
            // `Started`), so this is safe on every finish path.
            if let Some(win) = self.window.as_ref() {
                win.set_minimized(false);
                win.focus_window();
            }
        }

        // Flip any instances whose download job just finished from
        // `Pending` to `Ready` and persist. We match on the
        // version *string* of the most-recent job — same instance
        // can appear multiple times with different IDs, but the
        // status flips per-instance.
        let mut completed_versions: Vec<String> = Vec::new();
        for (vid, status) in self.downloads.iter_statuses() {
            if status.done && status.error.is_none() {
                completed_versions.push(vid.clone());
            }
        }
        if !completed_versions.is_empty() {
            let mut any_changed = false;
            for inst in self.instances.iter_mut() {
                if inst.status == ewo_render::screens::instances::InstanceStatus::Ready {
                    continue;
                }
                // Match by the version-string suffix on the meta
                // (commit_new_instance writes "<LOADER> · <version>").
                let v = match inst.version.rsplit(" · ").next() {
                    Some(s) => s,
                    None => &inst.version,
                };
                if completed_versions.iter().any(|w| w == v) {
                    inst.status = ewo_render::screens::instances::InstanceStatus::Ready;
                    any_changed = true;
                    log::info!(
                        "instances: \"{}\" → Ready (version {})",
                        inst.name, v
                    );
                }
            }
            if any_changed {
                persistence::save_instances(&self.instances);
            }
        }

        // Sync the live version manifest into the new-instance
        // modal's dropdown source. Filter to releases by default;
        // a "Show snapshots" toggle could later flip the second
        // arg. List goes from newest → oldest (Mojang's order).
        if let Some(manifest) = self.versions.manifest() {
            let want: Vec<String> = manifest
                .filtered_for_dropdown(false)
                .iter()
                .map(|e| e.id.clone())
                .collect();
            if want != self.modal.mc_versions {
                self.modal.apply_versions(want);
            }
        }

        // Account-tab actions — the press handler records one
        // request; dispatch it here, where we own `&mut auth`.
        if let Some(req) = self.prefs.account_request.take() {
            match req {
                AccountRequest::Add => {
                    log::info!("auth: add account -> interactive sign-in");
                    self.auth.start_interactive();
                }
                AccountRequest::SetActive(uuid) => {
                    self.auth.set_active(&uuid);
                }
                AccountRequest::Remove(uuid) => {
                    self.auth.remove(&uuid);
                }
                AccountRequest::OpenLauncherLink => {
                    log::info!("launcher-link modal: open");
                    self.social.clear_link_redeem();
                    self.launcher_link_modal.open();
                }
            }
        }

        // Profile-tab actions — switch / new / duplicate / delete.
        if let Some(req) = self.prefs.profile_request.take() {
            let applied = match req {
                ProfileRequest::Switch(name) => profile::switch(&name),
                ProfileRequest::New => {
                    let (_n, c, s) = profile::create();
                    Some((c, s))
                }
                ProfileRequest::Duplicate => {
                    profile::duplicate(&self.active_profile).map(|(_n, c, s)| (c, s))
                }
                ProfileRequest::Delete(name) => profile::delete(&name),
                ProfileRequest::Rename { index, new_name } => {
                    if let Some(old) = self.profiles.get(index).cloned() {
                        profile::rename(&old, &new_name);
                    }
                    self.prefs.profile_renaming = None;
                    self.prefs.profile_rename_buffer.clear();
                    None // a rename doesn't change the active config
                }
            };
            if let Some((config, settings)) = applied {
                self.apply_loaded_config(config, settings);
            }
            self.profiles = profile::list();
            self.active_profile = profile::active_name();
            // Keybinds are profile-scoped — the switched-to profile
            // carries its own set.
            self.keybinds = profile::load_keybinds();
        }

        // Keybinds-tab actions — arm a rebind or reset to defaults.
        if let Some(req) = self.prefs.keybind_request.take() {
            match req {
                KeybindRequest::Capture(idx) => {
                    if let Some(action) = keybind::REGISTRY.get(idx) {
                        log::info!("keybind: capturing for {}", action.id);
                        self.keybind_capture = Some(action.id.to_string());
                    }
                }
                KeybindRequest::ResetAll => {
                    for a in keybind::REGISTRY.iter() {
                        self.keybinds.insert(a.id.to_string(), a.default);
                    }
                    self.keybind_capture = None;
                    profile::save_keybinds(&self.keybinds);
                    log::info!("keybind: reset all to defaults");
                }
            }
        }

        // Reset preferences — wipe to bundled defaults, persist,
        // and resync the GL backend's vsync to match.
        if self.prefs.reset_requested {
            self.prefs.reset_requested = false;
            self.prefs
                .apply_config(&screens::SettingsConfig::default());
            profile::save(&self.prefs.to_config(), &self.settings);
            if let Some(b) = self.backend.as_ref() {
                b.set_vsync(self.prefs.vsync.on);
            }
            log::info!("reset_prefs: applied defaults");
        }

        // Modules tab — persist `modules.toml` when an edit landed.
        if self.prefs.modules_changed {
            self.prefs.modules_changed = false;
            let (enabled, fov) = self.prefs.modules_snapshot();
            profile::save_modules(&enabled, fov);
        }
        // PvP-Utils tab — persist `pvp.toml` when an edit landed. The
        // in-game mod polls the file's mtime each frame and reloads,
        // so a running game picks the change up immediately.
        if self.prefs.pvp_changed {
            self.prefs.pvp_changed = false;
            profile::save_pvp_config(&self.prefs.pvp);
        }
        if let Some(overlay) = self.dev_overlay.as_mut() {
            overlay.tick(dt);
            let density_changed = overlay.apply_to_settings(&mut self.settings);
            if density_changed {
                if let (Some(window), Some(backdrop)) =
                    (self.window.as_ref(), self.backdrop.as_mut())
                {
                    // Logical pixels — match `draw_frame`'s coord
                    // space (post-canvas-scale).
                    let size = window.inner_size();
                    let scale = window.scale_factor() as f32;
                    let logical_w = ((size.width as f32) / scale) as u32;
                    let logical_h = ((size.height as f32) / scale) as u32;
                    let (cw, ch) =
                        app_window::card_content_size(logical_w, logical_h);
                    backdrop.resize(cw, ch, &self.settings);
                }
            }
            // Mirror dev overlay's sim_error into the launching
            // state so the pbar variant matches what the dev pill
            // shows. Auto-starts a synthetic launch if needed so
            // the error has a bar to render against.
            if overlay.sim_error != self.launching.error {
                match overlay.sim_error {
                    Some(variant) => {
                        if self.launching.start_time.is_none() {
                            let (n, m) = self
                                .instances
                                .get(self.instance_prefs.selected)
                                .map(|i| {
                                    (
                                        i.name.clone(),
                                        format!(
                                            "{} · ADOPTIUM 21 · {} GB",
                                            i.version,
                                            self.instance_prefs.ram.value as i32,
                                        ),
                                    )
                                })
                                .unwrap_or_else(|| {
                                    (
                                        "Velvet Hours".to_string(),
                                        "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                                    )
                                });
                            self.launching.enter(time, &n, &m);
                        }
                        self.launching.trigger_error(variant, time);
                    }
                    None => self.launching.clear_error(),
                }
            }
        }
        if self.screen == Screen::Launching {
            self.launching.tick(time, dt);
            if self.launching.should_handoff(time) {
                log::info!("launching: handoff complete → returning to Instances");
                self.launching.exit();
                self.screen = Screen::Instances;
            }
        }

        let backdrop_ref = self.backdrop.as_ref();
        let fonts_ref = self.fonts.as_ref();
        let launch_button = self.launch_button;
        let menu_items = self.menu_items;
        let hovered_tab = self.hovered_tab;
        let back_link_hover = self.back_link_hover;
        let min_btn_hover = self.min_btn_hover;
        let close_btn_hover = self.close_btn_hover;
        // 0..1 entrance progress for the main menu (fade + slide).
        let main_menu_enter = if screen == Screen::MainMenu {
            ((time - self.screen_enter_at) / 0.34).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let settings_tab = self.settings_tab;
        let theme = &self.theme;
        let settings = &self.settings;
        let prefs = &self.prefs;
        let instance_prefs = &self.instance_prefs;
        let launching_state = &self.launching;
        let modal = &self.modal;
        let about_modal = &self.about_modal;
        let launcher_link_modal = &self.launcher_link_modal;
        let friends_prefs = &self.friends_prefs;
        // Phase H5: build owned per-row Vecs from the social
        // state. These stack locals outlive the friends_view
        // binding (same block scope), so the slices the view
        // holds borrow cleanly from them.
        let active_has_token = self
            .auth
            .active()
            .and_then(|a| self.auth.social_token(&a.uuid))
            .is_some();
        let (friends_rows, incoming_rows, outgoing_rows, friends_err_msg) =
            if active_has_token {
                match self.social.friends() {
                    social::FriendsListState::Loaded(list) => {
                        let f = list
                            .friends
                            .iter()
                            .map(friend_entry_to_view)
                            .collect::<Vec<_>>();
                        let i = list
                            .incoming
                            .iter()
                            .map(friend_entry_to_view)
                            .collect::<Vec<_>>();
                        let o = list
                            .outgoing
                            .iter()
                            .map(friend_entry_to_view)
                            .collect::<Vec<_>>();
                        (f, i, o, None)
                    }
                    social::FriendsListState::Failed(msg) => {
                        (vec![], vec![], vec![], Some(msg.clone()))
                    }
                    _ => (vec![], vec![], vec![], None),
                }
            } else {
                (vec![], vec![], vec![], None)
            };
        let friends_view: FriendsViewState<'_> = if !active_has_token {
            FriendsViewState::NotLinked
        } else if let Some(msg) = friends_err_msg.as_deref() {
            FriendsViewState::Failed(msg)
        } else if matches!(
            self.social.friends(),
            social::FriendsListState::Unknown
                | social::FriendsListState::Loading
        ) {
            FriendsViewState::Loading
        } else {
            FriendsViewState::Loaded {
                friends: &friends_rows,
                incoming: &incoming_rows,
                outgoing: &outgoing_rows,
            }
        };
        // Phase H2: clone the Failed message into a local so the
        // LinkRedeemView's &str doesn't borrow self.social (which
        // would conflict with `self.backend.as_mut()` below).
        let link_redeem_msg: Option<String> = match self.social.link_redeem() {
            social::LinkRedeemStatus::Failed(m) => Some(m.clone()),
            _ => None,
        };
        let link_redeem: LinkRedeemView<'_> =
            match (self.social.link_redeem(), link_redeem_msg.as_deref()) {
                (social::LinkRedeemStatus::Submitting, _) => LinkRedeemView::Submitting,
                (social::LinkRedeemStatus::Failed(_), Some(m)) => {
                    LinkRedeemView::Failed(m)
                }
                _ => LinkRedeemView::Idle,
            };
        let dev_overlay = self.dev_overlay.as_ref();
        let heading_hover = self.heading_hover;
        // Build the Account-tab view from the auth store. The row
        // Vec + the error string are stack locals that AccountView
        // borrows — keeps `ewo-render` ignorant of auth types.
        let active_uuid: Option<String> = self.auth.active().map(|a| a.uuid.clone());
        let account_rows: Vec<AccountRowView<'_>> = self
            .auth
            .accounts()
            .iter()
            .map(|a| AccountRowView {
                name: &a.name,
                uuid: &a.uuid,
                active: active_uuid.as_deref() == Some(a.uuid.as_str()),
            })
            .collect();
        let err_msg: Option<String> = if let AuthOp::Failed(err) = self.auth.op() {
            Some(format_auth_error(err))
        } else {
            None
        };
        let account_op = match self.auth.op() {
            AuthOp::Idle => AccountOpView::Idle,
            AuthOp::Working(stage) => AccountOpView::Working { stage: *stage },
            AuthOp::Failed(_) => AccountOpView::Failed {
                message: err_msg.as_deref().unwrap_or("auth failed"),
            },
        };
        let link_status = self.link_status_view();
        let account_view = AccountView {
            accounts: &account_rows,
            op: account_op,
            link_status,
        };
        let profile_rows: Vec<ProfileRowView<'_>> = self
            .profiles
            .iter()
            .map(|n| ProfileRowView {
                name: n,
                active: *n == self.active_profile,
            })
            .collect();
        let profile_view = ProfileView {
            profiles: &profile_rows,
        };
        // Keybinds-tab view — registry actions resolved against the
        // active profile's bindings. The chord labels are stack
        // locals the KeybindRowViews borrow.
        let keybind_chord_labels: Vec<String> = keybind::REGISTRY
            .iter()
            .map(|a| {
                self.keybinds
                    .get(a.id)
                    .copied()
                    .unwrap_or(a.default)
                    .label()
            })
            .collect();
        let keybind_rows: Vec<KeybindRowView<'_>> = keybind::REGISTRY
            .iter()
            .zip(&keybind_chord_labels)
            .map(|(a, label)| KeybindRowView {
                action_label: a.label,
                module: a.module,
                chord_label: label,
                capturing: self.keybind_capture.as_deref() == Some(a.id),
            })
            .collect();
        let keybind_view = KeybindView { rows: &keybind_rows };
        let frame_stats = FrameStats {
            fps: self.clock.avg_fps(),
            frame_ms: self.clock.avg_dt() * 1000.0,
            worst_ms: self.clock.worst_dt() * 1000.0,
        };
        let instances = self.instances.as_slice();
        // H6 — network-status widget view. Clone the snapshot into a
        // local so the borrowed `tps: &str` doesn't reference
        // self.social (which would conflict with self.backend.as_mut()
        // below — same reason as link_redeem_msg above).
        let server_status_snapshot = self.social.server_status().cloned();
        let server_widget_view = screens::ServerWidgetView {
            data: server_status_snapshot.as_ref().map(|s| screens::ServerWidgetData {
                online: s.online,
                online_count: s.online_count,
                max_players: s.max_players,
                tps: &s.tps,
            }),
            hovered: self.server_widget_hover,
        };
        // DPI handling: the GL surface is sized in physical pixels
        // (winit reports + we forward to `GlBackend::resize` raw).
        // But hit-testing in this file converts the cursor to
        // *logical* pixels (`cursor_card_local` divides by
        // `scale_factor`) so widget bounds in the screens crate are
        // already laid out against logical dimensions. To keep both
        // sides in the same coord space, we apply a one-time
        // `canvas.scale(scale, scale)` at the top of the frame and
        // pass `draw_frame` the *logical* viewport size. Without
        // this, on a HiDPI monitor (e.g. 1440p @ 125%) the renderer
        // would lay widgets out at physical pixel positions while
        // the hit-test sat in logical space — cursor would land
        // up-and-left of where the visible widget rendered.
        let scale = window.scale_factor() as f32;
        if let (Some(backend), Some(backdrop), Some(fonts)) =
            (self.backend.as_mut(), backdrop_ref, fonts_ref)
        {
            backend.render(|canvas, w, h| {
                let saved = canvas.save();
                canvas.scale((scale, scale));
                let w_lp = ((w as f32) / scale).round() as u32;
                let h_lp = ((h as f32) / scale).round() as u32;
                app_window::draw_frame(
                    canvas, backdrop, fonts, w_lp, h_lp, time, theme, settings,
                    screen, hovered_tab, &launch_button, &menu_items, settings_tab, prefs,
                    instance_prefs, launching_state, modal, about_modal,
                    launcher_link_modal, link_redeem, dev_overlay, frame_stats,
                    instances, heading_hover, account_view, profile_view, keybind_view,
                    friends_prefs, friends_view, server_widget_view,
                    main_menu_enter, back_link_hover, min_btn_hover, close_btn_hover,
                );
                canvas.restore_to_count(saved);
            });
        }
        // Chain the next redraw so animations keep ticking. The
        // unfocused-skip at the top of this arm short-circuits
        // before any expensive work, so leaving this on for the
        // unfocused path is cheap (one re-queue per ~100 ms while
        // unfocused, throttled by `about_to_wait`'s WaitUntil).
        if self.focused {
            window.request_redraw();
        }
    }
}
