//! Instances-screen clicks: the list ("+", ✎ rename, × delete, row select,
//! sort) and the detail panel's widgets.

use super::super::hit::{instances_list_click, InstancesListClick, InstancesListGeometry};
use super::*;

impl App {
    /// Steps 2.75 to 2.8 for the Instances screen. The list gets an
    /// unclaimed press first; the detail widgets' slider driver runs on
    /// every edge and mirrors a changed value into the instance; the detail
    /// press router runs on a still-unclaimed press. Returns whether the
    /// click is claimed after this step.
    pub(super) fn click_instances(&mut self, c: Click, mut handled: bool) -> bool {
        if self.screen != Screen::Instances {
            return handled;
        }
        if !handled && c.pressed {
            handled = self.click_instance_list(c);
        }
        let changed = drive_instance_widgets(
            &mut self.instance_prefs,
            &self.instances,
            self.fonts.as_ref(),
            c.pos,
            c.pressed,
            c.card_w,
            c.card_h,
        );
        if changed {
            sync_instance_config(&mut self.instances, &self.instance_prefs);
            persistence::save_instances(&self.instances);
        }
        if !handled && c.pressed {
            handled = handle_instances_press(
                &mut self.instance_prefs,
                &mut self.instances,
                self.fonts.as_ref(),
                c.pos,
                c.card_w,
                c.card_h,
            );
        }
        handled
    }

    /// The hit rects of the instance list at the current state. Without
    /// fonts only the "+" button can be placed.
    fn instances_list_geometry(&self, c: Click) -> InstancesListGeometry {
        let plus = screens::instances::add_button_bounds();
        let Some(fonts) = self.fonts.as_ref() else {
            return InstancesListGeometry {
                plus,
                rename: None,
                rows: Vec::new(),
                sort: None,
            };
        };
        InstancesListGeometry {
            plus,
            rename: screens::instances::rename_button_bounds(
                c.card_w,
                c.card_h,
                fonts,
                &self.instances,
                &self.instance_prefs,
            ),
            rows: screens::instances::list_row_bounds(
                c.card_h,
                fonts,
                &self.instances,
                &self.instance_prefs,
            ),
            sort: Some(screens::instances::sort_button_bounds(fonts)),
        }
    }

    /// A press on the instance list. Row hits arrive as display positions
    /// and map back to the underlying index through `display_order`.
    fn click_instance_list(&mut self, c: Click) -> bool {
        let geometry = self.instances_list_geometry(c);
        let Some(click) = instances_list_click(&geometry, c.pos) else {
            return false;
        };
        let underlying = |app: &App, display_idx: usize| {
            screens::instances::display_order(&app.instances, app.instance_prefs.sort_mode)
                [display_idx]
        };
        match click {
            InstancesListClick::Add => {
                log::info!("instances: + clicked → opening new-instance modal");
                self.modal.open();
            }
            InstancesListClick::Rename => {
                // Enter rename mode for the currently-selected instance.
                if let Some(inst) = self.instances.get(self.instance_prefs.selected) {
                    self.instance_prefs.renaming = true;
                    self.instance_prefs.rename_buffer = inst.name.clone();
                    self.instance_prefs.rename_focus_time = 0.0;
                    log::info!("rename: editing \"{}\"", inst.name);
                }
            }
            InstancesListClick::Delete(display_idx) => {
                // Ask first; `delete_flow` does the rest on confirmation.
                let idx = underlying(self, display_idx);
                self.open_delete_modal(idx);
            }
            InstancesListClick::Select(display_idx) => {
                let idx = underlying(self, display_idx);
                if idx != self.instance_prefs.selected {
                    log::info!(
                        "instances: select {} → {}",
                        self.instance_prefs.selected,
                        idx
                    );
                    self.instance_prefs.select(&self.instances, idx);
                    self.instance_prefs.selected_at = Some(self.clock.elapsed);
                }
            }
            InstancesListClick::CycleSort => {
                self.instance_prefs.sort_mode = self.instance_prefs.sort_mode.cycle();
                log::info!(
                    "instances: sort → {}",
                    self.instance_prefs.sort_mode.label()
                );
            }
        }
        true
    }
}
