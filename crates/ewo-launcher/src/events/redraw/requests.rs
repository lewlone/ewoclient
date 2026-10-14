//! Requests the UI recorded during input handling, dispatched once per frame
//! where `App` owns the services they need: account, profile, keybind and
//! reset requests, and the modules / PvP-Utils saves.

use super::*;

impl App {
    /// Dispatch the pending account / profile / keybind / reset requests and
    /// persist module and PvP-Utils edits.
    pub(super) fn apply_ui_requests(&mut self) {
        // Account-tab actions — the press handler records one
        // request; dispatch it here, where we own `&mut auth`.
        if let Some(req) = self.prefs.account_request.take() {
            match req {
                AccountRequest::Add => {
                    log::info!("auth: add account -> interactive sign-in");
                    self.auth.start_interactive();
                }
                AccountRequest::SetActive(uuid) => {
                    self.auth.set_active(&uuid);
                }
                AccountRequest::Remove(uuid) => {
                    self.auth.remove(&uuid);
                }
                AccountRequest::OpenLauncherLink => {
                    log::info!("launcher-link modal: open");
                    self.social.clear_link_redeem();
                    self.launcher_link_modal.open();
                }
            }
        }

        // Profile-tab actions — switch / new / duplicate / delete.
        if let Some(req) = self.prefs.profile_request.take() {
            let applied = match req {
                ProfileRequest::Switch(name) => profile::switch(&name),
                ProfileRequest::New => {
                    let (_n, c, s) = profile::create();
                    Some((c, s))
                }
                ProfileRequest::Duplicate => {
                    profile::duplicate(&self.active_profile).map(|(_n, c, s)| (c, s))
                }
                ProfileRequest::Delete(name) => profile::delete(&name),
                ProfileRequest::Rename { index, new_name } => {
                    if let Some(old) = self.profiles.get(index).cloned() {
                        profile::rename(&old, &new_name);
                    }
                    self.prefs.profile_renaming = None;
                    self.prefs.profile_rename_buffer.clear();
                    None // a rename doesn't change the active config
                }
            };
            if let Some((config, settings)) = applied {
                self.apply_loaded_config(config, settings);
            }
            self.profiles = profile::list();
            self.active_profile = profile::active_name();
            // Keybinds are profile-scoped — the switched-to profile
            // carries its own set.
            self.keybinds = profile::load_keybinds();
        }

        // Keybinds-tab actions — arm a rebind or reset to defaults.
        if let Some(req) = self.prefs.keybind_request.take() {
            match req {
                KeybindRequest::Capture(idx) => {
                    if let Some(action) = keybind::REGISTRY.get(idx) {
                        log::info!("keybind: capturing for {}", action.id);
                        self.keybind_capture = Some(action.id.to_string());
                    }
                }
                KeybindRequest::ResetAll => {
                    for a in keybind::REGISTRY.iter() {
                        self.keybinds.insert(a.id.to_string(), a.default);
                    }
                    self.keybind_capture = None;
                    profile::save_keybinds(&self.keybinds);
                    log::info!("keybind: reset all to defaults");
                }
            }
        }

        // Reset preferences — wipe to bundled defaults, persist,
        // and resync the GL backend's vsync to match.
        if self.prefs.reset_requested {
            self.prefs.reset_requested = false;
            self.prefs.apply_config(&screens::SettingsConfig::default());
            profile::save(&self.prefs.to_config(), &self.settings);
            if let Some(b) = self.backend.as_ref() {
                b.set_vsync(self.prefs.vsync.on);
            }
            log::info!("reset_prefs: applied defaults");
        }

        // Modules tab — persist `modules.toml` when an edit landed.
        if self.prefs.modules_changed {
            self.prefs.modules_changed = false;
            let (enabled, fov) = self.prefs.modules_snapshot();
            profile::save_modules(&enabled, fov);
        }
        // PvP-Utils tab — persist `pvp.toml` when an edit landed. The
        // in-game mod polls the file's mtime each frame and reloads,
        // so a running game picks the change up immediately.
        if self.prefs.pvp_changed {
            self.prefs.pvp_changed = false;
            profile::save_pvp_config(&self.prefs.pvp);
        }
    }
}
