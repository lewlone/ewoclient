//! `App` construction and the small state helpers the handlers share:
//! loading persisted config, applying a switched profile, the Frogsy link
//! status, and the main-menu widget / Launch-button geometry. Moved out of
//! `main.rs` unchanged.

use super::*;

/// The Account tab's Frogsy link line. `active_linked` is `None` with no
/// active account, otherwise whether that account holds a `social_token`.
/// `Hidden` when social isn't configured or there's no active account;
/// otherwise the stored token alone decides (no network probe, see
/// `social`'s module docs).
fn link_status(social_enabled: bool, active_linked: Option<bool>) -> LinkStatusView {
    match (social_enabled, active_linked) {
        (true, Some(true)) => LinkStatusView::Linked,
        (true, Some(false)) => LinkStatusView::LinkedNeedsLauncherLink,
        _ => LinkStatusView::Hidden,
    }
}

/// The main-menu network widget exists only while its status poll can run:
/// social configured and the active account linked.
fn server_widget_shown(social_enabled: bool, active_linked: bool) -> bool {
    social_enabled && active_linked
}

impl App {
    pub(crate) fn new(dev: bool) -> Self {
        // Phase F: settings live in the active client profile. `profile::load`
        // reconstructs the unified SettingsConfig + the cosmetic tokens,
        // migrating a pre-F settings.toml on first run.
        let (settings_config, settings) = profile::load();
        Self {
            window: None,
            backend: None,
            backdrop: None,
            fonts: None,
            clock: Clock::new(),
            motion_watch: ewo_core::motion::Watcher::new(),
            motion_recheck: false,
            theme: Theme::VELVET,
            settings,
            cursor: PhysicalPosition::new(0.0, 0.0),
            mouse_down: false,
            focused: true,
            occluded: false,
            last_activity: Instant::now(),
            screen: Screen::default(),
            settings_tab: SettingsTab::Graphics,
            prefs: {
                let mut p = Prefs::default();
                p.apply_config(&settings_config);
                let (mod_enabled, mod_fov) = profile::load_modules();
                p.apply_modules(&mod_enabled, mod_fov);
                // PvP-Utils config is shared with the in-game side via
                // `<profile>/pvp.toml`; load it here so the Settings tab
                // shows the user's current setup on first open.
                p.pvp = profile::load_pvp_config();
                p
            },
            // Try the persisted list first, fall back to the bundled
            // defaults if missing or malformed. An Ewo instance's stored
            // loader value is never used to fetch: the manifest URL is
            // resolved from the version and this machine's loader base at
            // launch (`loaders::base`), so there is nothing to re-key here.
            instances: persistence::load_instances(),
            instance_prefs: InstancePrefs::default(),
            launching: LaunchingState::default(),
            modal: NewInstanceModalState::default(),
            about_modal: AboutModalState::default(),
            delete_modal: DeleteInstanceModalState::default(),
            delete_job: None,
            launcher_link_modal: LauncherLinkModalState::default(),
            friends_prefs: FriendsPrefs::default(),
            dev_overlay: if dev {
                Some(DevOverlayState::default())
            } else {
                None
            },
            launch_button: VbtnState::default(),
            menu_items: [VbtnState::default(); 4],
            heading_hover: HoverGlowState::default(),
            // AuthService loads the persisted account store and kicks a
            // silent refresh for the active account itself (see `new`).
            auth: AuthService::new(),
            // SocialState reads `EWO_BOT_API_BASE`; unset = disabled.
            social: social::SocialState::new(),
            profiles: profile::list(),
            active_profile: profile::active_name(),
            keybinds: profile::load_keybinds(),
            keybind_capture: None,
            versions: versions::VersionService::new(),
            downloads: downloads::DownloadService::new(),
            launch_rx: None,
            active_launch: None,
            active_launch_started_at: 0.0,
            active_launch_window_seen: false,
            active_launch_marker_seen: false,
            active_launch_polled_at: None,
            active_launch_instance_id: None,
            pending_prepare: None,
            pending_minimize: None,
            runtime: runtime::RuntimeService::new(),
            pending_relaunch: None,
            pending_auth: None,
            jre_rescan_rx: None,
            jre_labels: screens::instances::java_runtime_options(&[]),
            jre_pins: Vec::new(),
            jre_labels_rx: None,
            runtime_log_bucket: 0,
            celebrate_until: None,
            active_server: None,
            server_widget_hover: false,
            hovered_tab: None,
            // Start settled (no entrance on first paint); transitions animate.
            prev_screen: Screen::default(),
            screen_enter_at: -1.0,
            back_link_hover: false,
            min_btn_hover: false,
            close_btn_hover: false,
            dev,
        }
    }

    /// Apply a freshly-loaded profile config — Settings widgets, cosmetic
    /// tokens, vsync, and the backdrop particle density (a profile-scoped
    /// token, so the pools re-spawn).
    pub(crate) fn apply_loaded_config(
        &mut self,
        config: screens::SettingsConfig,
        settings: Settings,
    ) {
        self.prefs.apply_config(&config);
        // Modules are per-profile too — reload them for the switched-to profile.
        let (mod_enabled, mod_fov) = profile::load_modules();
        self.prefs.apply_modules(&mod_enabled, mod_fov);
        // PvP-Utils is per-profile as well — reload pvp.toml.
        self.prefs.pvp = profile::load_pvp_config();
        self.settings = settings;
        if let Some(b) = self.backend.as_ref() {
            b.set_vsync(self.prefs.vsync.on);
        }
        if let (Some(window), Some(backdrop)) = (self.window.as_ref(), self.backdrop.as_mut()) {
            let scale = window.scale_factor();
            let size = window.inner_size();
            let cw = card_content_width(size, scale);
            let ch = card_content_height(size, scale);
            backdrop.resize(cw, ch, &self.settings);
        }
    }

    /// Map the active account's Frogsy link state into the renderer-side
    /// `LinkStatusView`. `Hidden` when social isn't configured or there's
    /// no active account; otherwise the stored `social_token` alone
    /// decides (no network probe, see `social`'s module docs): linked, or
    /// the clickable line that opens the launcher-link modal.
    pub(crate) fn link_status_view(&self) -> LinkStatusView {
        let active_linked = self
            .auth
            .active()
            .map(|a| self.auth.social_token(&a.uuid).is_some());
        link_status(self.social.enabled(), active_linked)
    }

    /// The active account's `social_token`, if it has one.
    pub(crate) fn active_social_token(&self) -> Option<String> {
        let active = self.auth.active()?;
        self.auth.social_token(&active.uuid).map(str::to_string)
    }

    /// Card-local bounds of the main-menu network widget, grown by the
    /// online-name strip when there are names to show. One source for the
    /// hover and click rects so they match what `draw_main_menu` paints.
    pub(crate) fn server_widget_rect(&self, card_w: f32, card_h: f32) -> skia_safe::Rect {
        let rows = self.social.server_status().map_or(0, |s| {
            screens::server_widget_name_rows(s.online, s.players.len())
        });
        screens::server_widget_bounds(card_w, card_h, rows)
    }

    /// Arriving at Friends refetches the list immediately rather than
    /// waiting for the next 30 s cadence tick. Goes through the same gate
    /// as every social request: with social disabled, or no linked
    /// account, `refresh_friends_now` issues nothing.
    pub(crate) fn refresh_friends_on_arrival(&mut self) {
        if let Some(token) = self.active_social_token() {
            self.social.refresh_friends_now(&token);
        }
    }

    /// The main-menu network widget exists only while its status poll can
    /// run: social configured and the active account linked.
    pub(crate) fn server_widget_visible(&self) -> bool {
        server_widget_shown(self.social.enabled(), self.active_social_token().is_some())
    }

    /// Bounds of the active screen's Launch button (card-local). Returns
    /// `None` for screens that don't have one.
    pub(crate) fn launch_button_bounds(&self, card_w: f32) -> Option<skia_safe::Rect> {
        match self.screen {
            Screen::Instances => Some(screens::instances::launch_button_bounds(card_w)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_status_is_hidden_without_social_or_an_account() {
        assert!(matches!(link_status(false, None), LinkStatusView::Hidden));
        assert!(matches!(
            link_status(false, Some(true)),
            LinkStatusView::Hidden
        ));
        assert!(matches!(
            link_status(false, Some(false)),
            LinkStatusView::Hidden
        ));
        assert!(matches!(link_status(true, None), LinkStatusView::Hidden));
    }

    #[test]
    fn link_status_follows_the_stored_token() {
        assert!(matches!(
            link_status(true, Some(true)),
            LinkStatusView::Linked
        ));
        assert!(matches!(
            link_status(true, Some(false)),
            LinkStatusView::LinkedNeedsLauncherLink
        ));
    }

    #[test]
    fn the_network_widget_needs_social_and_a_linked_account() {
        assert!(server_widget_shown(true, true));
        assert!(!server_widget_shown(true, false));
        assert!(!server_widget_shown(false, true));
        assert!(!server_widget_shown(false, false));
    }
}
