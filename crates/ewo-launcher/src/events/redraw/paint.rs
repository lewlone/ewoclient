//! Building the frame's views and drawing it through the Skia backend.

use super::views::*;
use super::*;

impl App {
    /// Build the per-frame views from launcher state and draw one frame.
    /// `screen` is the screen this frame draws (captured before the
    /// launching hand-off could change `self.screen`).
    ///
    /// The views borrow slices of `self` field by field, so `self.backend`
    /// can be borrowed mutably alongside them; anything that needs a `&self`
    /// method is computed first.
    pub(super) fn paint_frame(&mut self, window: &Arc<Window>, time: f32, screen: Screen) {
        let main_menu_enter = main_menu_enter(screen, time, self.screen_enter_at);
        let link_status = self.link_status_view();
        let server_widget_hidden = !self.server_widget_visible();

        // Phase H5: owned per-row friend data. It outlives the views that
        // borrow from it (same scope).
        let active_has_token = self
            .auth
            .active()
            .and_then(|a| self.auth.social_token(&a.uuid))
            .is_some();
        let friends = FriendsFrame::capture(&self.social, active_has_token, time);
        let friend_toast = friends.toast();
        let friends_view = friends.view(friend_toast.as_ref());

        let link_redeem = link_redeem_view(self.social.link_redeem());

        // The Account-tab view from the auth store. The row Vec and the
        // error string are locals that `AccountView` borrows, which keeps
        // `ewo-render` ignorant of auth types.
        let account_rows = account_rows(&self.auth);
        let auth_failure = auth_failure_text(self.auth.op());
        let account_view = AccountView {
            accounts: &account_rows,
            op: account_op_view(self.auth.op(), auth_failure.as_deref()),
            link_status,
        };
        let profile_rows = profile_rows(&self.profiles, &self.active_profile);
        let profile_view = ProfileView {
            profiles: &profile_rows,
        };
        // Keybinds-tab view: registry actions resolved against the active
        // profile's bindings.
        let keybind_chord_labels = keybind_chord_labels(&self.keybinds);
        let keybind_rows = keybind_rows(&keybind_chord_labels, self.keybind_capture.as_deref());
        let keybind_view = KeybindView {
            rows: &keybind_rows,
        };

        let frame_stats = FrameStats {
            fps: self.clock.avg_fps(),
            frame_ms: self.clock.avg_dt() * 1000.0,
            worst_ms: self.clock.worst_dt() * 1000.0,
        };
        // H6: the network-status widget view.
        let server_widget_view = server_widget_view(
            self.social.server_status(),
            self.server_widget_hover,
            server_widget_hidden,
            self.social.lobby_addr().is_some(),
        );

        let backdrop_ref = self.backdrop.as_ref();
        let fonts_ref = self.fonts.as_ref();
        let launch_button = self.launch_button;
        let menu_items = self.menu_items;
        let hovered_tab = self.hovered_tab;
        let back_link_hover = self.back_link_hover;
        let min_btn_hover = self.min_btn_hover;
        let close_btn_hover = self.close_btn_hover;
        let settings_tab = self.settings_tab;
        let theme = &self.theme;
        let settings = &self.settings;
        let prefs = &self.prefs;
        let instance_prefs = &self.instance_prefs;
        let launching_state = &self.launching;
        let modal = &self.modal;
        let about_modal = &self.about_modal;
        let delete_modal = &self.delete_modal;
        let launcher_link_modal = &self.launcher_link_modal;
        let friends_prefs = &self.friends_prefs;
        let dev_overlay = self.dev_overlay.as_ref();
        let heading_hover = self.heading_hover;
        let instances = self.instances.as_slice();
        // DPI handling: the GL surface is sized in physical pixels
        // (winit reports + we forward to `GlBackend::resize` raw).
        // But hit-testing converts the cursor to *logical* pixels
        // (`cursor_card_local` divides by `scale_factor`) so widget
        // bounds in the screens crate are already laid out against
        // logical dimensions. To keep both sides in the same coord
        // space, we apply a one-time `canvas.scale(scale, scale)` at the
        // top of the frame and pass `draw_frame` the *logical* viewport
        // size. Without this, on a HiDPI monitor (e.g. 1440p @ 125%)
        // the renderer would lay widgets out at physical pixel positions
        // while the hit-test sat in logical space — cursor would land
        // up-and-left of where the visible widget rendered.
        let scale = window.scale_factor() as f32;
        if let (Some(backend), Some(backdrop), Some(fonts)) =
            (self.backend.as_mut(), backdrop_ref, fonts_ref)
        {
            backend.render(|canvas, w, h| {
                let saved = canvas.save();
                canvas.scale((scale, scale));
                let w_lp = ((w as f32) / scale).round() as u32;
                let h_lp = ((h as f32) / scale).round() as u32;
                app_window::draw_frame(
                    canvas,
                    backdrop,
                    fonts,
                    w_lp,
                    h_lp,
                    time,
                    theme,
                    settings,
                    screen,
                    hovered_tab,
                    &launch_button,
                    &menu_items,
                    settings_tab,
                    prefs,
                    instance_prefs,
                    launching_state,
                    modal,
                    about_modal,
                    delete_modal,
                    launcher_link_modal,
                    link_redeem,
                    dev_overlay,
                    frame_stats,
                    instances,
                    heading_hover,
                    account_view,
                    profile_view,
                    keybind_view,
                    friends_prefs,
                    friends_view,
                    server_widget_view,
                    main_menu_enter,
                    back_link_hover,
                    min_btn_hover,
                    close_btn_hover,
                );
                canvas.restore_to_count(saved);
            });
        }
    }
}
