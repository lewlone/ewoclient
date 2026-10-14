//! The `--dev` tweaks overlay's pointer input. Moved out of `main.rs` unchanged.

use super::super::*;

/// Drive the dev overlay's slider drags + ghost button hover states.
pub(crate) fn drive_dev_overlay(
    overlay: &mut DevOverlayState,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) {
    for (slot, rect) in screens::dev_overlay::widget_bounds(card_w, card_h) {
        match slot {
            DevSlot::Reset => {
                overlay.reset_btn.handle(mouse, rect, false);
            }
            DevSlot::VsyncToggle => {
                overlay.vsync_btn.handle(mouse, rect, false);
            }
            DevSlot::SimError => {
                overlay.sim_error_btn.handle(mouse, rect, false);
            }
            slot => {
                if let Some(state) = screens::dev_overlay::slider_state_mut(overlay, slot) {
                    state.drive(mouse, rect, mouse_down);
                }
            }
        }
    }
}

/// Route a press inside the dev overlay panel. Returns whether the vsync
/// state changed this frame — caller uses that to call `GlBackend::set_vsync`.
pub(crate) fn handle_dev_overlay_press(
    overlay: &mut DevOverlayState,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> bool {
    for (slot, rect) in screens::dev_overlay::widget_bounds(card_w, card_h) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            DevSlot::Reset => {
                if overlay.reset_btn.handle(mouse, rect, true) {
                    log::info!("dev: reset to defaults");
                    overlay.reset_to_defaults();
                }
            }
            DevSlot::VsyncToggle => {
                if overlay.vsync_btn.handle(mouse, rect, true) {
                    overlay.vsync = !overlay.vsync;
                    log::info!("dev: vsync = {}", overlay.vsync);
                    return true;
                }
            }
            DevSlot::SimError if overlay.sim_error_btn.handle(mouse, rect, true) => {
                overlay.cycle_sim_error();
                log::info!("dev: sim_error = {:?}", overlay.sim_error);
            }
            // Sliders consumed by drive_dev_overlay.
            _ => {}
        }
        return false;
    }
    false
}
