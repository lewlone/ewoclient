//! Window chrome clicks: the minimize/close buttons, the `--dev` overlay and
//! the drag/resize fallback for clicks nothing else claimed.

use super::super::hit::{frame_action, window_button_at, FrameAction, WindowButton};
use super::*;
use winit::dpi::PhysicalSize;

impl App {
    /// Step -2: the top-right window buttons. Handled before everything
    /// (including modals and the dev overlay) so they always work; they are
    /// excluded from the drag caption in `hit_test`, so the press reaches
    /// here. Returns whether the click was consumed.
    pub(super) fn click_window_buttons(&mut self, event_loop: &ActiveEventLoop, c: Click) -> bool {
        if !c.pressed {
            return false;
        }
        match window_button_at(c.pos, c.card_w) {
            Some(WindowButton::Close) => {
                log::info!("window: close button → exit");
                event_loop.exit();
                true
            }
            Some(WindowButton::Minimize) => {
                log::info!("window: minimize button");
                if let Some(w) = self.window.as_ref() {
                    w.set_minimized(true);
                }
                true
            }
            None => false,
        }
    }

    /// Step -1: the dev overlay (`--dev`) sits above everything, including
    /// the modals, and absorbs input while the cursor is over its panel.
    pub(super) fn click_dev_overlay(&mut self, c: Click) -> bool {
        let Some(overlay) = self.dev_overlay.as_mut() else {
            return false;
        };
        let panel = screens::dev_overlay::panel_bounds(c.card_w, c.card_h);
        drive_dev_overlay(overlay, c.pos, c.pressed, c.card_w, c.card_h);
        if !rect_contains(&panel, c.pos) {
            return false;
        }
        if c.pressed {
            let vsync_changed = handle_dev_overlay_press(overlay, c.pos, c.card_w, c.card_h);
            if vsync_changed {
                if let Some(backend) = self.backend.as_ref() {
                    backend.set_vsync(overlay.vsync);
                }
            }
        }
        true
    }

    /// Step 4: a press nothing claimed drags the window from the caption,
    /// resizes from the border, or nudges the backdrop on plain content.
    pub(super) fn click_window_frame(
        &mut self,
        window: &Arc<Window>,
        size: PhysicalSize<u32>,
        scale: f64,
    ) {
        match frame_action(hit_test(self.cursor, size, scale)) {
            FrameAction::Drag => {
                let _ = window.drag_window();
            }
            FrameAction::Resize(dir) => {
                let _ = window.drag_resize_window(dir);
            }
            FrameAction::DisturbBackdrop => {
                if let Some(b) = self.backdrop.as_mut() {
                    b.disturb();
                }
            }
        }
    }
}
