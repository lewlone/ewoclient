//! Instances-screen detail widgets: slider drags and the press routing for the
//! detail panel. Moved out of `main.rs` unchanged.

use super::super::*;

/// Drive the Instances detail's slider drags + dropdown menu hover.
/// Mirror of `drive_settings_sliders` for the Instances screen. Returns
/// `true` when a slider value changed so the caller can mirror the
/// change into the underlying instance + persist.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_instance_widgets(
    prefs: &mut InstancePrefs,
    instances: &[Instance],
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };
    let mut value_changed = false;
    for (slot, rect) in screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances) {
        match slot {
            InstanceSlot::Ram => {
                if prefs.ram.drive(mouse, rect, mouse_down) {
                    value_changed = true;
                }
            }
            InstanceSlot::RenderDist => {
                if prefs.render_dist.drive(mouse, rect, mouse_down) {
                    value_changed = true;
                }
            }
            InstanceSlot::JavaRuntime | InstanceSlot::ModToggle(_) => {}
        }
    }

    // Update menu hover for any open dropdown.
    if let Some(slot) = prefs.open_dropdown() {
        // Immutable pass first (the options borrow prefs), then the
        // mutable hover update.
        let prep = screens::instances::dropdown_options(slot, prefs).and_then(|opts| {
            screens::instances::dropdown_head_for_slot(
                slot, fonts, card_w, card_h, prefs, instances,
            )
            .map(|head| {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                (menu_bounds, opts.len())
            })
        });
        if let Some((menu_bounds, n)) = prep {
            if let Some(state) = prefs.dropdown_state_mut(slot) {
                state.update_menu_hover(mouse, menu_bounds, n);
            }
        }
    }

    value_changed
}

/// Route a press event on the Instances screen — mirror of
/// `handle_settings_press`. Returns `true` when the press hit a widget.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_instances_press(
    prefs: &mut InstancePrefs,
    instances: &mut [Instance],
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };

    // (1) and (2): handle any open dropdown menu first.
    if let Some(open_slot) = prefs.open_dropdown() {
        // Immutable pass: head geometry + row count end before the
        // mutable state updates below.
        let prep = screens::instances::dropdown_options(open_slot, prefs).and_then(|opts| {
            screens::instances::dropdown_head_for_slot(
                open_slot, fonts, card_w, card_h, prefs, instances,
            )
            .map(|head| {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                (head, menu_bounds, opts.len())
            })
        });
        if let Some((head, menu_bounds, n)) = prep {
            let in_menu = rect_contains(&menu_bounds, mouse);
            let in_head = rect_contains(&head, mouse);
            if in_menu {
                let mut commit = false;
                if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                    if let Some(idx) = state.handle_menu(mouse, menu_bounds, n, true) {
                        log::info!(
                            "instance dropdown {:?} → {} ({})",
                            open_slot,
                            idx,
                            prefs
                                .jre_runtime_labels
                                .get(idx)
                                .map_or("?", String::as_str)
                        );
                        commit = true;
                    }
                }
                if commit {
                    sync_instance_config(instances, prefs);
                    crate::instance_ops::sync_java_pin(instances, prefs);
                    persistence::save_instances(instances);
                }
                return true;
            }
            if !in_head {
                if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                    state.close();
                }
            }
        }
    }

    // (3) Normal widget dispatch.
    for (slot, rect) in screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            InstanceSlot::JavaRuntime => {
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("instance dropdown {:?}: open={}", slot, state.open);
                    }
                }
            }
            InstanceSlot::ModToggle(i) => {
                if let Some(flag) = prefs.mods_on.get_mut(i) {
                    *flag = !*flag;
                    log::info!("mod[{}]: {}", i, *flag);
                    // Mirror the toggle into the underlying instance so
                    // the change persists and is reflected on the next
                    // selection-reset.
                    let new_value = *flag;
                    if let Some(inst) = instances.get_mut(prefs.selected) {
                        if let Some(m) = inst.mods.get_mut(i) {
                            m.on = new_value;
                        }
                    }
                    persistence::save_instances(instances);
                }
            }
            // Sliders consumed by drive_instance_widgets.
            InstanceSlot::Ram | InstanceSlot::RenderDist => {}
        }
        return true;
    }
    false
}
