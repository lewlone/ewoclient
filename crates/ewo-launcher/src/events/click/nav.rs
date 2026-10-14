//! Navigation clicks: the top tab bar, the "‹ Main menu" back-link and the
//! Settings sidebar tabs.

use super::super::hit::{back_link_hit, tab_at};
use super::*;

impl App {
    /// Step 1: the tab bar, which has priority over everything on screen.
    pub(super) fn click_tab_bar(&mut self, c: Click) -> bool {
        if !c.pressed {
            return false;
        }
        let target = self
            .fonts
            .as_ref()
            .and_then(|fonts| tab_at(screens::tab_bounds(c.card_w, fonts), c.pos));
        let Some(target) = target else {
            return false;
        };
        if self.screen != target {
            self.navigate_via_tab(target, c.time);
        }
        true
    }

    /// Switch to `target` from a tab click, abandoning the old screen's
    /// transient UI state (open dropdowns, rename/capture modes, the modal).
    fn navigate_via_tab(&mut self, target: Screen, time: f32) {
        log::info!("nav: {:?} → {:?}", self.screen, target);
        self.screen = target;
        if target == Screen::Friends {
            self.refresh_friends_on_arrival();
        }
        self.launch_button = VbtnState::default();
        for s in self.menu_items.iter_mut() {
            *s = VbtnState::default();
        }
        self.prefs.close_dropdowns();
        self.instance_prefs.close_dropdowns();
        self.keybind_capture = None;
        self.prefs.keybind_request = None;
        self.prefs.profile_renaming = None;
        self.prefs.profile_rename_buffer.clear();
        self.modal.close();
        // Trigger the tab fade-in when arriving at Settings, so the active
        // tab's content greets the user with the same animation it plays when
        // they switch tabs.
        if target == Screen::Settings {
            self.prefs.tab_changed_at = Some(time);
            self.prefs.settings_scroll = 0.0;
        }
        // Demo affordance: clicking the LAUNCHING tab without an active
        // launch kicks off a fresh synthetic one so the screen is never empty.
        if target == Screen::Launching && self.launching.start_time.is_none() {
            let (inst_name, inst_meta) = self.demo_launch_label();
            self.launching.enter(time, &inst_name, &inst_meta);
        }
    }

    /// Step 1.5: the "‹ Main menu" back-link on the screen-head screens
    /// navigates home.
    pub(super) fn click_back_link(&mut self, c: Click) -> bool {
        if !c.pressed || !back_link_hit(self.screen, c.pos) {
            return false;
        }
        log::info!("nav: {:?} → MainMenu (back-link)", self.screen);
        self.screen = Screen::MainMenu;
        self.prefs.close_dropdowns();
        self.instance_prefs.close_dropdowns();
        true
    }

    /// Step 2.5: the Settings sidebar's tab switch.
    pub(super) fn click_settings_sidebar(&mut self, c: Click) -> bool {
        if !c.pressed || self.screen != Screen::Settings {
            return false;
        }
        let tab = self
            .fonts
            .as_ref()
            .and_then(|fonts| tab_at(screens::settings::sidebar_tab_bounds(fonts), c.pos));
        let Some(tab) = tab else {
            return false;
        };
        if self.settings_tab != tab {
            log::info!("settings: {:?} → {:?}", self.settings_tab, tab);
            self.settings_tab = tab;
            self.prefs.close_dropdowns();
            // A tab switch abandons a pending keybind capture or an
            // in-progress profile rename.
            self.keybind_capture = None;
            self.prefs.keybind_request = None;
            self.prefs.profile_renaming = None;
            self.prefs.profile_rename_buffer.clear();
            self.prefs.tab_changed_at = Some(c.time);
            self.prefs.settings_scroll = 0.0;
        }
        true
    }
}
