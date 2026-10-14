//! `WindowEvent::KeyboardInput` (press): keyboard focus routing. Exactly one
//! owner takes each key, chosen by the pure [`key_target`]; the owner's
//! method applies it. Text fields share [`push_text`].

use super::*;
use winit::keyboard::{PhysicalKey, SmolStr};

/// What currently wants keyboard input. Collected from `App` so the routing
/// order below can be tested without one.
#[derive(Clone, Copy, Debug, Default)]
struct KeyFocus {
    /// The Keybinds tab armed a rebind.
    keybind_capture: bool,
    instance_renaming: bool,
    profile_renaming: bool,
    about_open: bool,
    link_modal_open: bool,
    /// On the Friends screen with the add-friend input focused.
    friends_add_focused: bool,
    modal_open: bool,
    modal_name_focused: bool,
    /// The pressed key is Escape.
    escape: bool,
}

/// Who owns a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyTarget {
    KeybindCapture,
    InstanceRename,
    ProfileRename,
    /// Esc with the About modal open.
    AboutClose,
    LinkModal,
    FriendsAdd,
    /// Esc with the new-instance modal open.
    ModalClose,
    ModalName,
}

/// First match wins, in this order: a pending rebind, the instance rename,
/// the profile rename, Esc in About, the launcher-link modal, the
/// add-friend input, Esc in the new-instance modal, its name field.
/// Anything else leaves the key unhandled.
fn key_target(f: KeyFocus) -> Option<KeyTarget> {
    if f.keybind_capture {
        Some(KeyTarget::KeybindCapture)
    } else if f.instance_renaming {
        Some(KeyTarget::InstanceRename)
    } else if f.profile_renaming {
        Some(KeyTarget::ProfileRename)
    } else if f.escape && f.about_open {
        Some(KeyTarget::AboutClose)
    } else if f.link_modal_open {
        Some(KeyTarget::LinkModal)
    } else if f.friends_add_focused {
        Some(KeyTarget::FriendsAdd)
    } else if f.escape && f.modal_open {
        Some(KeyTarget::ModalClose)
    } else if f.modal_open && f.modal_name_focused {
        Some(KeyTarget::ModalName)
    } else {
        None
    }
}

/// Append the chars of `text` that `accept` allows, stopping at `max_chars`
/// characters in `buf`. Returns whether anything was appended (so the caller
/// restarts the caret blink).
fn push_text(
    buf: &mut String,
    text: &str,
    max_chars: usize,
    accept: impl Fn(char) -> bool,
) -> bool {
    let mut pushed = false;
    for ch in text.chars() {
        if accept(ch) && buf.chars().count() < max_chars {
            buf.push(ch);
            pushed = true;
        }
    }
    pushed
}

/// Characters that can't go in a profile name (it becomes a directory under
/// `profiles/`).
const PROFILE_NAME_FORBIDDEN: &str = "/\\:*?\"<>|";

fn is_profile_name_char(ch: char) -> bool {
    !ch.is_control() && !PROFILE_NAME_FORBIDDEN.contains(ch)
}

impl App {
    /// A key press (`WindowEvent::KeyboardInput`, pressed).
    pub(crate) fn on_key_pressed(
        &mut self,
        window: &Arc<Window>,
        logical_key: Key,
        physical_key: PhysicalKey,
        text: Option<SmolStr>,
    ) {
        let escape = logical_key == Key::Named(NamedKey::Escape);
        let focus = KeyFocus {
            keybind_capture: self.keybind_capture.is_some(),
            instance_renaming: self.instance_prefs.renaming,
            profile_renaming: self.prefs.profile_renaming.is_some(),
            about_open: self.about_modal.open,
            link_modal_open: self.launcher_link_modal.open,
            friends_add_focused: self.screen == Screen::Friends && self.friends_prefs.add_focused,
            modal_open: self.modal.open,
            modal_name_focused: self.modal.name_focused,
            escape,
        };
        match key_target(focus) {
            Some(KeyTarget::KeybindCapture) => {
                self.key_keybind_capture(&logical_key, physical_key);
                window.request_redraw();
            }
            Some(KeyTarget::InstanceRename) => self.key_instance_rename(&logical_key, text),
            Some(KeyTarget::ProfileRename) => self.key_profile_rename(&logical_key, text),
            Some(KeyTarget::AboutClose) => {
                log::info!("about: Esc → closing");
                self.about_modal.close();
            }
            Some(KeyTarget::LinkModal) => self.key_link_modal(&logical_key, text.as_ref()),
            Some(KeyTarget::FriendsAdd) => self.key_friends_add(&logical_key, text.as_ref()),
            Some(KeyTarget::ModalClose) => {
                log::info!("modal: Esc → closing");
                self.modal.close();
            }
            Some(KeyTarget::ModalName) => self.key_modal_name(&logical_key, text),
            None => {}
        }
    }

    /// Keybind capture — the Keybinds tab armed a rebind, so the next key
    /// press becomes the new binding. Esc cancels; a key GLFW can't name
    /// leaves the rebind armed.
    fn key_keybind_capture(&mut self, logical_key: &Key, physical_key: PhysicalKey) {
        let Some(action_id) = self.keybind_capture.clone() else {
            return;
        };
        if *logical_key == Key::Named(NamedKey::Escape) {
            log::info!("keybind: capture cancelled");
            self.keybind_capture = None;
        } else if let PhysicalKey::Code(code) = physical_key {
            if let Some(chord) = keybind::KeyChord::from_winit(code) {
                log::info!("keybind: {} → {}", action_id, chord.label());
                self.keybinds.insert(action_id, chord);
                profile::save_keybinds(&self.keybinds);
                self.keybind_capture = None;
            }
        }
    }

    /// Inline rename for the selected instance.
    fn key_instance_rename(&mut self, logical_key: &Key, text: Option<SmolStr>) {
        const RENAME_MAX_LEN: usize = 48;
        if *logical_key == Key::Named(NamedKey::Escape) {
            log::info!("rename: Esc → cancelled");
            self.instance_prefs.renaming = false;
            self.instance_prefs.rename_buffer.clear();
        } else if *logical_key == Key::Named(NamedKey::Enter) {
            // Display name only — the folder is keyed by the instance's
            // stable `id`, so a rename moves nothing.
            match persistence::validate_instance_name(&self.instance_prefs.rename_buffer) {
                Ok(trimmed) => {
                    let trimmed = trimmed.to_string();
                    if let Some(inst) = self.instances.get_mut(self.instance_prefs.selected) {
                        log::info!("rename: \"{}\" → \"{}\"", inst.name, trimmed);
                        inst.name = trimmed;
                    }
                    persistence::save_instances(&self.instances);
                }
                Err(e) => log::info!("rename: rejected ({:?}) → cancelled", e),
            }
            self.instance_prefs.renaming = false;
            self.instance_prefs.rename_buffer.clear();
        } else if *logical_key == Key::Named(NamedKey::Backspace) {
            self.instance_prefs.rename_buffer.pop();
            self.instance_prefs.rename_focus_time = 0.0;
        } else if let Some(t) = text {
            if push_text(
                &mut self.instance_prefs.rename_buffer,
                &t,
                RENAME_MAX_LEN,
                |ch| !ch.is_control(),
            ) {
                self.instance_prefs.rename_focus_time = 0.0;
            }
        }
    }

    /// Inline rename for a client profile (Settings → Profiles).
    fn key_profile_rename(&mut self, logical_key: &Key, text: Option<SmolStr>) {
        if *logical_key == Key::Named(NamedKey::Escape) {
            log::info!("profile rename: Esc → cancelled");
            self.prefs.profile_renaming = None;
            self.prefs.profile_rename_buffer.clear();
        } else if *logical_key == Key::Named(NamedKey::Enter) {
            if let Some(idx) = self.prefs.profile_renaming {
                self.prefs.profile_request = Some(ProfileRequest::Rename {
                    index: idx,
                    new_name: self.prefs.profile_rename_buffer.clone(),
                });
            }
            self.prefs.profile_renaming = None;
        } else if *logical_key == Key::Named(NamedKey::Backspace) {
            self.prefs.profile_rename_buffer.pop();
            self.prefs.profile_rename_focus_time = 0.0;
        } else if let Some(t) = text {
            if push_text(
                &mut self.prefs.profile_rename_buffer,
                &t,
                profile::MAX_NAME_LEN,
                is_profile_name_char,
            ) {
                self.prefs.profile_rename_focus_time = 0.0;
            }
        }
    }

    /// Phase H2 launcher-link modal — captures Esc, Enter, Backspace, and
    /// digit chars while open. Non-digit chars are ignored (`push_digit`
    /// double-checks).
    fn key_link_modal(&mut self, logical_key: &Key, text: Option<&SmolStr>) {
        if *logical_key == Key::Named(NamedKey::Escape) {
            log::info!("launcher-link modal: Esc → closing");
            self.launcher_link_modal.close();
            self.social.clear_link_redeem();
        } else if *logical_key == Key::Named(NamedKey::Backspace) {
            self.launcher_link_modal.pop_digit();
        } else if *logical_key == Key::Named(NamedKey::Enter) && self.launcher_link_modal.is_ready()
        {
            let code = self.launcher_link_modal.code.clone();
            log::info!("launcher-link modal: Enter → submitting");
            let uuid = self.auth.active().map(|a| a.uuid.clone());
            self.social.submit_link_code(code, uuid.as_deref());
        } else if let Some(t) = text {
            for ch in t.chars() {
                if ch.is_ascii_digit() {
                    self.launcher_link_modal.push_digit(ch);
                }
            }
        }
    }

    /// Phase H5 — text input for the add-friend MC name.
    fn key_friends_add(&mut self, logical_key: &Key, text: Option<&SmolStr>) {
        const NAME_MAX_LEN: usize = 32;
        if *logical_key == Key::Named(NamedKey::Escape) {
            self.friends_prefs.add_focused = false;
        } else if *logical_key == Key::Named(NamedKey::Backspace) {
            self.friends_prefs.add_buffer.pop();
            self.friends_prefs.add_focus_time = 0.0;
        } else if *logical_key == Key::Named(NamedKey::Enter) {
            let token = self.active_social_token();
            let name = self.friends_prefs.add_buffer.trim().to_string();
            if let (Some(token), false) = (token, name.is_empty()) {
                log::info!("friends: Enter → request '{}'", name);
                self.social.submit_friend_request_by_name(&token, name);
                self.friends_prefs.add_buffer.clear();
                self.friends_prefs.add_focused = false;
            }
        } else if let Some(t) = text {
            // MC names are ASCII alphanumeric + underscore, 1-16 chars, but
            // accept broader and let the bot reject — keeps the input
            // forgiving.
            if push_text(&mut self.friends_prefs.add_buffer, t, NAME_MAX_LEN, |ch| {
                !ch.is_control()
            }) {
                self.friends_prefs.add_focus_time = 0.0;
            }
        }
    }

    /// New-instance modal name field: append printable chars, pop on
    /// backspace. Only when the name field is the active focus target (set
    /// by clicking the input).
    fn key_modal_name(&mut self, logical_key: &Key, text: Option<SmolStr>) {
        const NAME_MAX_LEN: usize = 48;
        if *logical_key == Key::Named(NamedKey::Backspace) {
            self.modal.name.pop();
            self.modal.name_focus_time = 0.0; // restart caret blink
            if !self.modal.name.is_empty() {
                self.modal.name_error = false;
            }
        } else if *logical_key == Key::Named(NamedKey::Enter) {
            // Enter commits — same as clicking Create.
            if let Some(form) = try_submit_modal(&mut self.modal) {
                commit_new_instance(
                    &mut self.instances,
                    &mut self.instance_prefs,
                    &self.versions,
                    &mut self.downloads,
                    form,
                    self.clock.elapsed,
                );
                self.modal.close();
            }
        } else if let Some(t) = text {
            if push_text(&mut self.modal.name, &t, NAME_MAX_LEN, |ch| {
                !ch.is_control()
            }) {
                self.modal.name_focus_time = 0.0;
                self.modal.name_error = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus() -> KeyFocus {
        KeyFocus::default()
    }

    #[test]
    fn nothing_focused_leaves_keys_unhandled() {
        assert_eq!(key_target(focus()), None);
        assert_eq!(
            key_target(KeyFocus {
                escape: true,
                ..focus()
            }),
            None
        );
    }

    #[test]
    fn a_pending_rebind_beats_every_other_owner() {
        let all = KeyFocus {
            keybind_capture: true,
            instance_renaming: true,
            profile_renaming: true,
            about_open: true,
            link_modal_open: true,
            friends_add_focused: true,
            modal_open: true,
            modal_name_focused: true,
            escape: true,
        };
        assert_eq!(key_target(all), Some(KeyTarget::KeybindCapture));
        // Peel owners off one by one: the documented priority order.
        let order = [
            KeyTarget::InstanceRename,
            KeyTarget::ProfileRename,
            KeyTarget::AboutClose,
            KeyTarget::LinkModal,
            KeyTarget::FriendsAdd,
            KeyTarget::ModalClose,
        ];
        let mut f = KeyFocus {
            keybind_capture: false,
            ..all
        };
        for expect in order {
            assert_eq!(key_target(f), Some(expect));
            match expect {
                KeyTarget::InstanceRename => f.instance_renaming = false,
                KeyTarget::ProfileRename => f.profile_renaming = false,
                KeyTarget::AboutClose => f.about_open = false,
                KeyTarget::LinkModal => f.link_modal_open = false,
                KeyTarget::FriendsAdd => f.friends_add_focused = false,
                KeyTarget::ModalClose => f.escape = false,
                _ => unreachable!(),
            }
        }
        assert_eq!(key_target(f), Some(KeyTarget::ModalName));
    }

    #[test]
    fn about_only_takes_escape_and_the_modal_name_needs_focus() {
        let about = KeyFocus {
            about_open: true,
            ..focus()
        };
        assert_eq!(key_target(about), None, "other keys fall through About");
        assert_eq!(
            key_target(KeyFocus {
                escape: true,
                ..about
            }),
            Some(KeyTarget::AboutClose)
        );
        let modal = KeyFocus {
            modal_open: true,
            ..focus()
        };
        assert_eq!(key_target(modal), None, "name field not focused");
        assert_eq!(
            key_target(KeyFocus {
                modal_name_focused: true,
                ..modal
            }),
            Some(KeyTarget::ModalName)
        );
        assert_eq!(
            key_target(KeyFocus {
                escape: true,
                modal_name_focused: true,
                ..modal
            }),
            Some(KeyTarget::ModalClose),
            "Esc closes before it edits"
        );
    }

    #[test]
    fn the_link_modal_swallows_all_keys_when_open() {
        let f = KeyFocus {
            link_modal_open: true,
            ..focus()
        };
        assert_eq!(key_target(f), Some(KeyTarget::LinkModal));
        // ...but a rename in progress is checked first.
        assert_eq!(
            key_target(KeyFocus {
                profile_renaming: true,
                ..f
            }),
            Some(KeyTarget::ProfileRename)
        );
    }

    #[test]
    fn push_text_filters_and_caps_by_character_count() {
        let mut buf = String::new();
        assert!(push_text(&mut buf, "ab\ncd", 4, |c| !c.is_control()));
        assert_eq!(buf, "abcd");
        // Full: nothing more, and no change reported.
        assert!(!push_text(&mut buf, "e", 4, |c| !c.is_control()));
        assert_eq!(buf, "abcd");
        // The cap counts chars, not bytes.
        let mut wide = String::new();
        assert!(push_text(&mut wide, "ééé", 2, |_| true));
        assert_eq!(wide, "éé");
        // Nothing accepted → no change reported.
        let mut empty = String::new();
        assert!(!push_text(&mut empty, "\t\n", 8, |c| !c.is_control()));
        assert!(empty.is_empty());
    }

    #[test]
    fn profile_names_reject_path_characters() {
        for bad in ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\n'] {
            assert!(!is_profile_name_char(bad), "{bad:?}");
        }
        for good in ['a', 'Z', '7', ' ', '-', '_', 'é', '.'] {
            assert!(is_profile_name_char(good), "{good:?}");
        }
        let mut buf = String::new();
        push_text(&mut buf, "my/pro:file*", 32, is_profile_name_char);
        assert_eq!(buf, "myprofile");
    }
}
