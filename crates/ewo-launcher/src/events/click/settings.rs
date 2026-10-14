//! Settings-screen clicks: the active tab's widgets.

use super::*;

impl App {
    /// Step 2.7: Settings widget interaction. Toggles flip on the press
    /// edge; sliders begin a drag (the drag continues in `CursorMoved` via
    /// `drive_settings_sliders`); dropdown heads toggle their menu open or
    /// closed; clicks on open menu rows commit the selection; clicks
    /// elsewhere close the menu. The slider/hover driver runs on every edge
    /// (even a claimed click); the press router only on an unclaimed press.
    /// Returns whether the click is claimed after this step.
    pub(super) fn click_settings_body(&mut self, c: Click, mut handled: bool) -> bool {
        if self.screen != Screen::Settings {
            return handled;
        }
        let account_uuids = self.auth.account_uuids();
        let link_status = self.link_status_view();
        let mut changed = drive_settings_sliders(
            &mut self.prefs,
            self.settings_tab,
            self.fonts.as_ref(),
            c.pos,
            c.pressed,
            c.card_w,
            c.card_h,
            &account_uuids,
            &self.profiles,
            link_status,
        );
        if !handled && c.pressed {
            let (h, press_changed) = handle_settings_press(
                &mut self.prefs,
                self.settings_tab,
                self.fonts.as_ref(),
                c.pos,
                c.card_w,
                c.card_h,
                &account_uuids,
                &self.profiles,
                link_status,
            );
            handled = h;
            changed = changed || press_changed;
        }
        if changed {
            profile::save(&self.prefs.to_config(), &self.settings);
            if let Some(b) = self.backend.as_ref() {
                b.set_vsync(self.prefs.vsync.on);
            }
        }
        handled
    }
}
