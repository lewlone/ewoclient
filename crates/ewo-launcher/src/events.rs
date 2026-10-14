//! The launcher window's event handlers, one method per large
//! `WindowEvent` arm. `App::window_event` (`main.rs`) is the dispatcher.

use super::*;

mod click;
mod hit;
mod hover;
mod redraw;
#[cfg(test)]
mod test_support;
mod widgets;
use widgets::*;

impl App {
    /// A key press (`WindowEvent::KeyboardInput`, pressed).
    pub(crate) fn on_key_pressed(
        &mut self,
        window: &Arc<Window>,
        logical_key: Key,
        physical_key: winit::keyboard::PhysicalKey,
        text: Option<winit::keyboard::SmolStr>,
    ) {
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
                match persistence::validate_instance_name(&self.instance_prefs.rename_buffer) {
                    Ok(trimmed) => {
                        let trimmed = trimmed.to_string();
                        if let Some(inst) = self.instances.get_mut(self.instance_prefs.selected) {
                            log::info!("rename: \"{}\" → \"{}\"", inst.name, trimmed);
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
                        && self.prefs.profile_rename_buffer.chars().count() < profile::MAX_NAME_LEN
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
                let uuid = self.auth.active().map(|a| a.uuid.clone());
                self.social.submit_link_code(code, uuid.as_deref());
            } else if let Some(t) = text.as_ref() {
                for ch in t.chars() {
                    if ch.is_ascii_digit() {
                        self.launcher_link_modal.push_digit(ch);
                    }
                }
            }
        } else if self.screen == Screen::Friends && self.friends_prefs.add_focused {
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
                        && self.friends_prefs.add_buffer.chars().count() < NAME_MAX_LEN
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
                        let layout =
                            screens::new_instance_modal::compute_layout(card_w, card_h, fonts);
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
                                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
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
                        // Immutable pass first (the options borrow
                        // prefs), then the mutable scroll update.
                        let prep = screens::instances::dropdown_options(slot, &self.instance_prefs)
                            .and_then(|opts| {
                                screens::instances::dropdown_head_for_slot(
                                    slot,
                                    fonts,
                                    card_w,
                                    card_h,
                                    &self.instance_prefs,
                                    &self.instances,
                                )
                                .map(|head| {
                                    let (menu_bounds, _flip) =
                                        ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                                    (menu_bounds, opts.len())
                                })
                            });
                        if let Some((menu_bounds, n)) = prep {
                            if let Some(state) = self.instance_prefs.dropdown_state_mut(slot) {
                                state.scroll_by(dy, menu_bounds, n);
                            }
                            handled = true;
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
                    let max_scroll =
                        screens::instances::list_max_scroll(card_h, fonts, &self.instances);
                    self.instance_prefs.list_scroll =
                        (self.instance_prefs.list_scroll + dy).clamp(0.0, max_scroll);
                } else {
                    let panel = screens::instances::detail_panel_bounds(card_w, card_h);
                    if rect_contains(&panel, card_pos) {
                        let max_scroll = screens::instances::detail_max_scroll(
                            card_w,
                            card_h,
                            fonts,
                            &self.instance_prefs,
                            &self.instances,
                        );
                        self.instance_prefs.detail_scroll =
                            (self.instance_prefs.detail_scroll + dy).clamp(0.0, max_scroll);
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
                self.prefs.settings_scroll = (self.prefs.settings_scroll + dy).clamp(0.0, max);
            }
        }
    }
}
