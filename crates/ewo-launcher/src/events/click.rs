//! Left-button click routing. `App::on_left_mouse` builds a [`Click`] and
//! walks the layers top to bottom: window buttons, dev overlay and the
//! modals absorb the click when they own it; otherwise the screen content
//! gets it, first handler to claim it winning. The pure "what was hit"
//! rules live in `events::hit`; the per-area handlers in the submodules
//! apply them to `App`.

use super::*;

mod chrome;
mod friends;
mod instances;
mod launching;
mod menu;
mod modals;
mod nav;
mod settings;

/// One mouse-button edge, in card-local logical pixels.
#[derive(Clone, Copy, Debug)]
struct Click {
    pos: (f32, f32),
    /// `true` on the press edge, `false` on release.
    pressed: bool,
    card_w: f32,
    card_h: f32,
    /// Launcher clock, seconds.
    time: f32,
}

impl App {
    /// `WindowEvent::MouseInput` for the left button.
    pub(crate) fn on_left_mouse(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: &Arc<Window>,
        state: ElementState,
    ) {
        let pressed = matches!(state, ElementState::Pressed);
        self.mouse_down = pressed;

        let scale = window.scale_factor();
        let size = window.inner_size();
        let c = Click {
            pos: cursor_card_local(self.cursor, scale),
            pressed,
            card_w: card_content_width(size, scale),
            card_h: card_content_height(size, scale),
            time: self.clock.elapsed,
        };

        // Layers above the screens. Each one that owns the click ends it:
        // the window buttons always work; the dev overlay sits above the
        // modals; an open modal absorbs all input.
        if self.click_window_buttons(event_loop, c)
            || self.click_dev_overlay(c)
            || self.click_about_modal(c)
            || self.click_link_modal(c)
            || self.click_new_instance_modal(c)
        {
            return;
        }

        if !self.click_screen(event_loop, c) && pressed {
            self.click_window_frame(window, size, scale);
        }
    }

    /// The screen content. Returns whether a handler claimed the click (so
    /// it must not fall through to the window drag/resize fallback).
    /// Handlers run in priority order; the Settings and Instances widget
    /// drivers also run when the click was already claimed, because slider
    /// drags track the button state, not the press edge.
    fn click_screen(&mut self, event_loop: &ActiveEventLoop, c: Click) -> bool {
        let mut handled = self.click_tab_bar(c);
        if !handled {
            handled = self.click_back_link(c);
        }
        if !handled {
            handled = self.click_main_menu(event_loop, c);
        }
        if !handled {
            handled = self.click_settings_sidebar(c);
        }
        if !handled {
            handled = self.click_friends(c);
        }
        handled = self.click_settings_body(c, handled);
        handled = self.click_instances(c, handled);
        if !handled {
            handled = self.click_launching_footer(c);
        }
        if !handled {
            handled = self.click_launch_button(c);
        }
        handled
    }
}
