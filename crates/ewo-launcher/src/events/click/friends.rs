//! Friends-screen clicks (Phase H5): the link button, the add-friend input
//! and button, and the per-row Accept / Decline / Remove / Join buttons.

use super::super::hit::{friends_click, FriendsClick};
use super::*;

/// What a friend-row button does once resolved against the cached list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowAction {
    /// H6: launch into the friend's current server.
    Join {
        addr: String,
    },
    Accept {
        discord_id: String,
    },
    Decline {
        discord_id: String,
    },
    Remove {
        discord_id: String,
    },
}

/// Resolve a row button against the list. `None` when the row is gone
/// (the list changed under the cursor) or, for Join, the friend has no
/// server address. Accept and Decline index the incoming requests; Remove
/// and Join index the friends.
fn row_action(
    kind: screens::RowButtonKind,
    index: usize,
    list: &social::FriendsList,
) -> Option<RowAction> {
    use screens::RowButtonKind as K;
    match kind {
        K::Join => list
            .friends
            .get(index)
            .and_then(|e| e.presence.as_ref())
            .and_then(|p| p.server_addr.clone())
            .map(|addr| RowAction::Join { addr }),
        K::Accept => list.incoming.get(index).map(|e| RowAction::Accept {
            discord_id: e.discord_id.clone(),
        }),
        K::Decline => list.incoming.get(index).map(|e| RowAction::Decline {
            discord_id: e.discord_id.clone(),
        }),
        K::Remove => list.friends.get(index).map(|e| RowAction::Remove {
            discord_id: e.discord_id.clone(),
        }),
    }
}

impl App {
    /// Step 2.7 (Phase H5): Friends screen press handling. Inline-sized
    /// surface: a handful of click targets plus the row buttons. With social
    /// off the screen shows a notice and there is nothing to click.
    pub(super) fn click_friends(&mut self, c: Click) -> bool {
        if !c.pressed || self.screen != Screen::Friends {
            return false;
        }
        let Some(fonts) = self.fonts.as_ref() else {
            return false;
        };
        let counts = if let social::FriendsListState::Loaded(list) = self.social.friends() {
            screens::FriendsCounts {
                friends: list.friends.len(),
                incoming: list.incoming.len(),
                outgoing: list.outgoing.len(),
            }
        } else {
            screens::FriendsCounts::default()
        };
        let layout = screens::friends_layout(c.card_w, fonts, counts);
        let token = self.active_social_token();
        let click = friends_click(&layout, c.pos, self.social.enabled(), token.is_some());

        match click {
            FriendsClick::Nothing => false,
            FriendsClick::OpenLinkModal => {
                // NotLinked: same path as the rose line in Settings → Account.
                log::info!("friends: link button → open launcher-link modal");
                self.social.clear_link_redeem();
                self.launcher_link_modal.open();
                true
            }
            FriendsClick::FocusAddInput => {
                self.friends_prefs.add_focused = true;
                self.friends_prefs.add_focus_time = 0.0;
                true
            }
            FriendsClick::SubmitAdd => {
                let name = self.friends_prefs.add_buffer.trim().to_string();
                if let (Some(token), false) = (token.as_deref(), name.is_empty()) {
                    log::info!("friends: + Add → request '{}'", name);
                    self.social.submit_friend_request_by_name(token, name);
                    self.friends_prefs.add_buffer.clear();
                    self.friends_prefs.add_focused = false;
                }
                true
            }
            FriendsClick::Defocus => {
                // Click landed away from the input; lose focus.
                self.friends_prefs.add_focused = false;
                false
            }
            FriendsClick::Row { kind, index } => {
                self.friends_prefs.add_focused = false;
                let Some(token) = token.as_deref() else {
                    return false;
                };
                self.apply_friend_row(kind, index, token, c.time)
            }
        }
    }

    /// Run a row button. Join always claims the click (even with no address
    /// to join); the others only when their row still exists.
    fn apply_friend_row(
        &mut self,
        kind: screens::RowButtonKind,
        index: usize,
        token: &str,
        time: f32,
    ) -> bool {
        let action = match self.social.friends() {
            social::FriendsListState::Loaded(list) => row_action(kind, index, list),
            _ => None,
        };
        let claimed = action.is_some() || kind == screens::RowButtonKind::Join;
        match action {
            Some(RowAction::Join { addr }) => {
                log::info!("friends: join row {} → {}", index, addr);
                self.start_launch(self.instance_prefs.selected, Some(addr), time);
            }
            Some(RowAction::Accept { discord_id }) => {
                log::info!("friends: accept {}", discord_id);
                self.social.respond_friend_request(token, discord_id, true);
            }
            Some(RowAction::Decline { discord_id }) => {
                log::info!("friends: decline {}", discord_id);
                self.social.respond_friend_request(token, discord_id, false);
            }
            Some(RowAction::Remove { discord_id }) => {
                log::info!("friends: remove {}", discord_id);
                self.social.remove_friend(token, discord_id);
            }
            None => {}
        }
        claimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screens::RowButtonKind as K;
    use social::{FriendEntry, FriendPresence, FriendsList};

    fn entry(id: &str, server: Option<&str>) -> FriendEntry {
        FriendEntry {
            discord_id: id.into(),
            minecraft_uuid: None,
            presence: server.map(|s| FriendPresence {
                location: "in_game".into(),
                server_addr: Some(s.into()),
                screen: None,
            }),
        }
    }
    fn list() -> FriendsList {
        FriendsList {
            friends: vec![entry("f0", Some("a:1")), entry("f1", None)],
            incoming: vec![entry("i0", None)],
            outgoing: vec![entry("o0", None)],
        }
    }

    #[test]
    fn accept_and_decline_target_the_incoming_request() {
        assert_eq!(
            row_action(K::Accept, 0, &list()),
            Some(RowAction::Accept {
                discord_id: "i0".into()
            })
        );
        assert_eq!(
            row_action(K::Decline, 0, &list()),
            Some(RowAction::Decline {
                discord_id: "i0".into()
            })
        );
    }

    #[test]
    fn remove_targets_the_friend_not_the_request() {
        assert_eq!(
            row_action(K::Remove, 1, &list()),
            Some(RowAction::Remove {
                discord_id: "f1".into()
            })
        );
    }

    #[test]
    fn join_needs_the_friends_server_address() {
        assert_eq!(
            row_action(K::Join, 0, &list()),
            Some(RowAction::Join { addr: "a:1".into() })
        );
        // Friend 1 has no presence at all.
        assert_eq!(row_action(K::Join, 1, &list()), None);
    }

    #[test]
    fn a_row_that_vanished_resolves_to_nothing() {
        for kind in [K::Accept, K::Decline, K::Remove, K::Join] {
            assert_eq!(row_action(kind, 9, &list()), None, "{kind:?}");
            assert_eq!(
                row_action(kind, 0, &FriendsList::default()),
                None,
                "{kind:?}"
            );
        }
    }
}
