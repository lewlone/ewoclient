//! `WindowEvent::CursorMoved`: hover state for every hit target, then the
//! mouse cursor icon. The pure cursor choices are unit-tested here; the hit
//! rules shared with clicking live in `events::hit`.

use super::hit::{back_link_hit, tab_at};
use super::*;
use winit::dpi::PhysicalSize;

/// One cursor position in card-local logical pixels.
#[derive(Clone, Copy, Debug)]
struct Hover {
    pos: (f32, f32),
    card_w: f32,
    card_h: f32,
    /// Launcher clock, seconds.
    time: f32,
    /// A modal (new-instance, About or launcher-link) is open and absorbs
    /// hover: background widgets must not react under its shroud.
    any_modal: bool,
}

/// The cursor over the new-instance modal: I-beam on the name field,
/// pointer on its other controls, the default arrow elsewhere (shroud).
fn modal_cursor_icon(over_name_input: bool, over_widget: bool) -> CursorIcon {
    if over_name_input {
        CursorIcon::Text
    } else if over_widget {
        CursorIcon::Pointer
    } else {
        CursorIcon::Default
    }
}

/// An open modal's cursor wins; otherwise a pointer over any clickable
/// target; otherwise `None`, meaning "fall back to the window-edge cursor".
fn pick_cursor(modal: Option<CursorIcon>, over_pointer_target: bool) -> Option<CursorIcon> {
    modal.or(over_pointer_target.then_some(CursorIcon::Pointer))
}

impl App {
    /// `WindowEvent::CursorMoved`.
    pub(crate) fn on_cursor_moved(
        &mut self,
        window: &Arc<Window>,
        position: PhysicalPosition<f64>,
    ) {
        self.cursor = position;
        let scale = window.scale_factor();
        let size = window.inner_size();
        let h = Hover {
            pos: cursor_card_local(position, scale),
            card_w: card_content_width(size, scale),
            card_h: card_content_height(size, scale),
            time: self.clock.elapsed,
            any_modal: self.modal.open || self.about_modal.open || self.launcher_link_modal.open,
        };

        self.hover_chrome(h);
        self.hover_screen_widgets(h);
        self.hover_launch_and_friends_buttons(h);
        let hovering_menu = self.hover_main_menu(h);
        self.hover_modal_buttons(h);
        self.update_pointer_icon(window, size, scale, h, hovering_menu);
    }

    /// Tab bar, back-link and window-button hover.
    fn hover_chrome(&mut self, h: Hover) {
        let Hover {
            pos: card_pos,
            card_w,
            any_modal,
            ..
        } = h;
        // Top tab bar hover → glow (every screen). Suppressed while a
        // modal is open so tabs don't light up under the shroud.
        self.hovered_tab = if any_modal {
            None
        } else {
            self.fonts
                .as_ref()
                .and_then(|fonts| tab_at(screens::tab_bounds(card_w, fonts), card_pos))
        };
        // "‹ Main menu" back-link hover (for its glow).
        self.back_link_hover = !any_modal && back_link_hit(self.screen, card_pos);
        // Top-right window buttons hover.
        let (min_btn, close_btn) = app_window::window_button_bounds(card_w);
        self.min_btn_hover = rect_contains(&min_btn, card_pos);
        self.close_btn_hover = rect_contains(&close_btn, card_pos);
    }

    /// The open modal, or the active screen's widgets and lists, plus the
    /// dev overlay.
    fn hover_screen_widgets(&mut self, h: Hover) {
        let Hover {
            pos: card_pos,
            card_w,
            card_h,
            ..
        } = h;
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
                self.instance_prefs.rename_hover = screens::instances::rename_button_bounds(
                    card_w,
                    card_h,
                    fonts,
                    &self.instances,
                    &self.instance_prefs,
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
                self.instance_prefs.add_hover =
                    rect_contains(&screens::instances::add_button_bounds(), card_pos);
                self.instance_prefs.sort_hover =
                    rect_contains(&screens::instances::sort_button_bounds(fonts), card_pos);
            }
        } else {
            // Clear lingering hover state when off the Instances screen.
            self.instance_prefs.list_hover = None;
            self.instance_prefs.add_hover = false;
        }
        if let Some(overlay) = self.dev_overlay.as_mut() {
            drive_dev_overlay(overlay, card_pos, self.mouse_down, card_w, card_h);
        }
    }

    /// Launch button and the Friends screen's buttons.
    fn hover_launch_and_friends_buttons(&mut self, h: Hover) {
        let Hover {
            pos: card_pos,
            card_w,
            time,
            any_modal,
            ..
        } = h;
        // Update launch-button + main-menu hover state. Modal-open
        // suppresses these so background buttons don't react under
        // a modal (new-instance, About, or launcher-link).
        if any_modal {
            self.launch_button = VbtnState::default();
        } else if let Some(b) = self.launch_button_bounds(card_w) {
            self.launch_button
                .update(card_pos, b, self.mouse_down, time);
        } else {
            self.launch_button = VbtnState::default();
        }

        // Friends-screen buttons hover (link-launcher / add-friend) —
        // without this the buttons never animate and read as dead.
        if !any_modal && self.screen == Screen::Friends {
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
    }

    /// Main-menu items, heading glow and network widget. Returns whether the
    /// cursor is over a menu item.
    fn hover_main_menu(&mut self, h: Hover) -> bool {
        let Hover {
            pos: card_pos,
            card_w,
            card_h,
            time,
            any_modal,
        } = h;
        let mut hovering_menu = false;
        if !any_modal && self.screen == Screen::MainMenu {
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
            // H6 — network widget hover (lower-left card). The widget
            // only exists once social is configured and linked.
            let server_rect = self.server_widget_rect(card_w, card_h);
            self.server_widget_hover =
                self.server_widget_visible() && rect_contains(&server_rect, card_pos);
        } else {
            for s in self.menu_items.iter_mut() {
                *s = VbtnState::default();
            }
            // Clear heading-hover when off the main menu so the
            // glow doesn't survive a screen change.
            self.heading_hover.update(false, time);
            self.server_widget_hover = false;
        }

        hovering_menu
    }

    /// The About and launcher-link modals' button hover.
    fn hover_modal_buttons(&mut self, h: Hover) {
        let Hover {
            pos: card_pos,
            card_w,
            card_h,
            time,
            ..
        } = h;
        // About modal — drive the Close button hover so the ghost
        // glow tracks the cursor without needing a click.
        if self.about_modal.open {
            let close_rect = screens::about_modal::close_button_bounds(card_w, card_h);
            self.about_modal
                .close_btn
                .handle(card_pos, close_rect, false);
        }

        // Phase H2 launcher-link modal — drive Cancel + Submit hover.
        if self.launcher_link_modal.open {
            let cancel_rect = screens::launcher_link_modal::cancel_button_bounds(card_w, card_h);
            let submit_rect = screens::launcher_link_modal::submit_button_bounds(card_w, card_h);
            self.launcher_link_modal
                .cancel_btn
                .handle(card_pos, cancel_rect, false);
            self.launcher_link_modal.submit_btn.update(
                card_pos,
                submit_rect,
                self.mouse_down,
                time,
            );
        }
    }

    /// Choose and set the mouse cursor icon.
    fn update_pointer_icon(
        &mut self,
        window: &Arc<Window>,
        size: PhysicalSize<u32>,
        scale: f64,
        h: Hover,
        hovering_menu: bool,
    ) {
        let Hover {
            pos: card_pos,
            card_w,
            card_h,
            ..
        } = h;
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
                screens::settings::widget_bounds(self.settings_tab, fonts, card_w, card_h)
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
                    card_w,
                    card_h,
                    fonts,
                    &self.instance_prefs,
                    &self.instances,
                );
                let on_widget = widgets.iter().any(|(_, r)| rect_contains(r, card_pos));
                let row_rects = screens::instances::list_row_bounds(
                    card_h,
                    fonts,
                    &self.instances,
                    &self.instance_prefs,
                );
                let on_list_row = row_rects.iter().any(|r| rect_contains(r, card_pos));
                let on_add = rect_contains(&screens::instances::add_button_bounds(), card_pos);
                let on_sort =
                    rect_contains(&screens::instances::sort_button_bounds(fonts), card_pos);
                // × buttons are inside row rects, but we want the
                // pointer cursor regardless — `on_list_row` already
                // covers them.
                let on_rename = screens::instances::rename_button_bounds(
                    card_w,
                    card_h,
                    fonts,
                    &self.instances,
                    &self.instance_prefs,
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
                let layout = screens::new_instance_modal::compute_layout(card_w, card_h, fonts);
                let over_name_input = rect_contains(&layout.name_input, card_pos);
                let over_widget = !over_name_input
                    && screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                        .iter()
                        .any(|(_, r)| rect_contains(r, card_pos));
                Some(modal_cursor_icon(over_name_input, over_widget))
            } else {
                Some(CursorIcon::Default)
            }
        } else {
            None
        };

        let over_pointer_target = hovering_tab
            || hovering_menu
            || hovering_settings_tab
            || hovering_settings_widget
            || hovering_instance_widget
            || self.launch_button.hover;
        match pick_cursor(modal_cursor, over_pointer_target) {
            Some(icon) => window.set_cursor(icon),
            None => update_cursor_icon(window, &self.cursor, size, scale),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modal_cursor_prefers_text_then_pointer_then_arrow() {
        assert_eq!(modal_cursor_icon(true, false), CursorIcon::Text);
        assert_eq!(modal_cursor_icon(false, true), CursorIcon::Pointer);
        assert_eq!(modal_cursor_icon(false, false), CursorIcon::Default);
    }

    #[test]
    fn an_open_modal_decides_the_cursor_over_everything_else() {
        assert_eq!(
            pick_cursor(Some(CursorIcon::Text), true),
            Some(CursorIcon::Text)
        );
        // The modal's arrow beats a pointer target under the shroud.
        assert_eq!(
            pick_cursor(Some(CursorIcon::Default), true),
            Some(CursorIcon::Default)
        );
        assert_eq!(pick_cursor(None, true), Some(CursorIcon::Pointer));
        assert_eq!(
            pick_cursor(None, false),
            None,
            "falls back to the edge cursor"
        );
    }
}
