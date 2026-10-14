//! Clicks on the three modals. When a modal is open it absorbs all input:
//! each handler returns `true` whenever its modal is open, so nothing
//! underneath ever sees the click.

use super::super::hit::{about_dismissal, link_modal_action, Dismiss, LinkModalAction};
use super::*;

impl App {
    /// Step 0a: the About modal. Its Close button or a shroud press closes
    /// it; a press elsewhere inside the card is a no-op, so the modal can't
    /// be dismissed by a misclick on the card itself.
    pub(super) fn click_about_modal(&mut self, c: Click) -> bool {
        if !self.about_modal.open {
            return false;
        }
        let close_rect = screens::about_modal::close_button_bounds(c.card_w, c.card_h);
        let close_clicked = self
            .about_modal
            .close_btn
            .handle(c.pos, close_rect, c.pressed);
        let on_shroud = screens::about_modal::shroud_consumes(c.pos, c.card_w, c.card_h);
        match about_dismissal(close_clicked, c.pressed, on_shroud) {
            Some(Dismiss::Button) => {
                log::info!("about: Close clicked");
                self.about_modal.close();
            }
            Some(Dismiss::Shroud) => {
                log::info!("about: shroud click → closing");
                self.about_modal.close();
            }
            None => {}
        }
        true
    }

    /// Step 0b (Phase H2): the launcher-link modal. Cancel closes and clears
    /// the redeem state; Submit fires `submit_link_code` once the code is
    /// ready; a shroud press closes.
    pub(super) fn click_link_modal(&mut self, c: Click) -> bool {
        if !self.launcher_link_modal.open {
            return false;
        }
        let cancel_rect = screens::launcher_link_modal::cancel_button_bounds(c.card_w, c.card_h);
        let submit_rect = screens::launcher_link_modal::submit_button_bounds(c.card_w, c.card_h);
        let cancel_clicked =
            self.launcher_link_modal
                .cancel_btn
                .handle(c.pos, cancel_rect, c.pressed);
        let submit_clicked =
            self.launcher_link_modal
                .submit_btn
                .update(c.pos, submit_rect, self.mouse_down, c.time);
        let on_shroud = screens::launcher_link_modal::shroud_consumes(c.pos, c.card_w, c.card_h);
        let action = link_modal_action(
            cancel_clicked,
            submit_clicked,
            self.launcher_link_modal.is_ready(),
            c.pressed,
            on_shroud,
        );
        match action {
            LinkModalAction::Cancel => {
                log::info!("launcher-link modal: Cancel clicked");
                self.launcher_link_modal.close();
                self.social.clear_link_redeem();
            }
            LinkModalAction::Submit => {
                let code = self.launcher_link_modal.code.clone();
                log::info!("launcher-link modal: Submit clicked");
                let uuid = self.auth.active().map(|a| a.uuid.clone());
                self.social.submit_link_code(code, uuid.as_deref());
            }
            LinkModalAction::Shroud => {
                log::info!("launcher-link modal: shroud click → closing");
                self.launcher_link_modal.close();
                self.social.clear_link_redeem();
            }
            LinkModalAction::Nothing => {}
        }
        true
    }

    /// Step 0: the new-instance modal. `drive_modal_widgets` runs first so
    /// slider drags start on the rising edge inside bounds;
    /// `handle_modal_press` runs after to consume button / dropdown / shroud
    /// clicks.
    pub(super) fn click_new_instance_modal(&mut self, c: Click) -> bool {
        if !self.modal.open {
            return false;
        }
        drive_modal_widgets(
            &mut self.modal,
            self.fonts.as_ref(),
            c.pos,
            c.pressed,
            c.card_w,
            c.card_h,
        );
        if c.pressed {
            handle_modal_press(
                &mut self.modal,
                &mut self.instances,
                &mut self.instance_prefs,
                &self.versions,
                &mut self.downloads,
                self.fonts.as_ref(),
                c.pos,
                c.card_w,
                c.card_h,
                c.time,
            );
        }
        true
    }
}
