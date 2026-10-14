//! `WindowEvent::MouseWheel`: scroll routing. An open dropdown menu gets the
//! wheel first; an open modal absorbs the rest; otherwise the active
//! screen's primary scrollable region scrolls.

use super::*;

/// Logical pixels one wheel line scrolls. `LineDelta` lines are
/// platform-dependent; this is a roughly right tick-to-pixels mapping.
const WHEEL_LINE_PX: f32 = 32.0;

/// Card x below which the cursor is over the Instances "Worlds" list column
/// (right of it is the detail panel).
const WORLDS_COLUMN_RIGHT: f32 = 320.0;

/// The scroll offset delta for a wheel event, in logical pixels, positive
/// when scrolling down (content moves up). `PixelDelta` is physical, so it
/// is divided by the scale factor.
fn wheel_dy(delta: MouseScrollDelta, scale: f64) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINE_PX,
        MouseScrollDelta::PixelDelta(p) => -(p.y as f32 / scale as f32),
    }
}

/// The furthest a region can scroll: its content minus its viewport, never
/// negative.
fn max_scroll(content_h: f32, visible_h: f32) -> f32 {
    (content_h - visible_h).max(0.0)
}

/// `current + dy`, kept inside `0..=max`.
fn scrolled(current: f32, dy: f32, max: f32) -> f32 {
    (current + dy).clamp(0.0, max)
}

/// Which Instances region the wheel scrolls at a card-local x.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstancesPane {
    WorldsList,
    /// The detail panel (only when the cursor is inside it).
    Detail,
}

fn instances_pane(x: f32) -> InstancesPane {
    if x < WORLDS_COLUMN_RIGHT {
        InstancesPane::WorldsList
    } else {
        InstancesPane::Detail
    }
}

impl App {
    /// `WindowEvent::MouseWheel`.
    pub(crate) fn on_mouse_wheel(&mut self, window: &Arc<Window>, delta: MouseScrollDelta) {
        let scale = window.scale_factor();
        let card_pos = cursor_card_local(self.cursor, scale);
        let size = window.inner_size();
        let card_w = card_content_width(size, scale);
        let card_h = card_content_height(size, scale);
        let dy = wheel_dy(delta, scale);

        // Routing priority:
        //  1. If a dropdown menu is open *anywhere*, route the
        //     wheel to it (so users can scroll the open list).
        //  2. If the modal is open, absorb the wheel even when no
        //     menu is open — prevents the underlying Instances
        //     panel from scrolling beneath a modal.
        //  3. Otherwise, scroll the active screen's primary
        //     scrollable region (the Instances list or detail panel,
        //     or a Settings tab's list).

        // About modal absorbs wheel events outright — there's nothing
        // scrollable inside it, but we don't want the Instances panel
        // to scroll behind the dialog.
        if self.about_modal.open || self.delete_modal.open {
            return;
        }

        let mut handled = self.scroll_open_dropdown(dy, card_w, card_h);

        // Modal absorbs all remaining wheel events so the
        // background doesn't scroll beneath it.
        if !handled && self.modal.open {
            handled = true;
        }

        if !handled && self.screen == Screen::Instances {
            self.scroll_instances(dy, card_pos, card_w, card_h);
        }
        if !handled && self.screen == Screen::Settings {
            self.scroll_settings_list(dy, card_w, card_h);
        }
    }

    /// Route the wheel to the open dropdown menu: the new-instance modal's
    /// while it is open, otherwise the active screen's. Returns whether a
    /// menu took it.
    fn scroll_open_dropdown(&mut self, dy: f32, card_w: f32, card_h: f32) -> bool {
        let Some(fonts) = self.fonts.as_ref() else {
            return false;
        };
        let mut handled = false;
        // Modal dropdown takes precedence when modal is open.
        if self.modal.open {
            if let Some(slot) = self.modal.open_dropdown() {
                // Extract just the count so the `Vec<&str>` borrow on
                // `self.modal` ends before the `&mut VdropState`.
                let opt_count = self.modal.dropdown_options(slot).map(|v| v.len());
                if let Some(opt_count) = opt_count {
                    let layout = screens::new_instance_modal::compute_layout(card_w, card_h, fonts);
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
                        if let Some(head) =
                            screens::settings::dropdown_head_for_slot(slot, fonts, card_w, card_h)
                        {
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
        handled
    }

    /// Fall-through: the Worlds list (left) or the Instances detail panel
    /// (right) scrolls, depending on which side the cursor is on.
    fn scroll_instances(&mut self, dy: f32, card_pos: (f32, f32), card_w: f32, card_h: f32) {
        let Some(fonts) = self.fonts.as_ref() else {
            return;
        };
        match instances_pane(card_pos.0) {
            InstancesPane::WorldsList => {
                let max = screens::instances::list_max_scroll(card_h, fonts, &self.instances);
                self.instance_prefs.list_scroll =
                    scrolled(self.instance_prefs.list_scroll, dy, max);
            }
            InstancesPane::Detail => {
                let panel = screens::instances::detail_panel_bounds(card_w, card_h);
                if rect_contains(&panel, card_pos) {
                    let max = screens::instances::detail_max_scroll(
                        card_w,
                        card_h,
                        fonts,
                        &self.instance_prefs,
                        &self.instances,
                    );
                    self.instance_prefs.detail_scroll =
                        scrolled(self.instance_prefs.detail_scroll, dy, max);
                }
            }
        }
    }

    /// Settings — scroll the Keybinds / Modules / PvP-Utils tab list.
    fn scroll_settings_list(&mut self, dy: f32, card_w: f32, card_h: f32) {
        let Some(fonts) = self.fonts.as_ref() else {
            return;
        };
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
        self.prefs.settings_scroll = scrolled(
            self.prefs.settings_scroll,
            dy,
            max_scroll(content_h, visible_h),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn wheel_lines_scroll_32px_and_down_is_positive() {
        assert_eq!(wheel_dy(MouseScrollDelta::LineDelta(0.0, -1.0), 2.0), 32.0);
        assert_eq!(wheel_dy(MouseScrollDelta::LineDelta(0.0, 2.0), 1.0), -64.0);
        assert_eq!(wheel_dy(MouseScrollDelta::LineDelta(5.0, 0.0), 1.0), 0.0);
    }

    #[test]
    fn wheel_pixels_are_physical_and_divide_by_the_scale() {
        let px = |y: f64| MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, y));
        assert_eq!(wheel_dy(px(-100.0), 1.0), 100.0);
        assert_eq!(wheel_dy(px(-100.0), 2.0), 50.0);
        assert_eq!(wheel_dy(px(60.0), 1.5), -40.0);
    }

    #[test]
    fn scrolling_stays_inside_the_content() {
        assert_eq!(max_scroll(500.0, 200.0), 300.0);
        assert_eq!(max_scroll(100.0, 200.0), 0.0, "content shorter than view");
        assert_eq!(scrolled(0.0, -50.0, 300.0), 0.0);
        assert_eq!(scrolled(290.0, 50.0, 300.0), 300.0);
        assert_eq!(scrolled(100.0, 32.0, 300.0), 132.0);
        // Nothing to scroll: pinned at the top whatever the wheel does.
        assert_eq!(scrolled(0.0, 32.0, 0.0), 0.0);
    }

    #[test]
    fn the_instances_wheel_splits_at_the_worlds_column() {
        assert_eq!(instances_pane(0.0), InstancesPane::WorldsList);
        assert_eq!(instances_pane(319.9), InstancesPane::WorldsList);
        assert_eq!(instances_pane(320.0), InstancesPane::Detail);
        assert_eq!(instances_pane(900.0), InstancesPane::Detail);
    }
}
