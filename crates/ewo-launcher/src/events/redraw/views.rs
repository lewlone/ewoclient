//! Builders for the per-frame view-models `draw_frame` borrows: friends rows
//! and toast, account / profile / keybind rows, the network widget and the
//! link-redeem state. They take narrow slices of `App` (not `&App`) so the
//! caller can hold them while it borrows `self.backend` mutably.

use super::*;
use std::collections::BTreeMap;

/// Which Friends-screen state to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FriendsKind {
    /// No social API configured.
    Disabled,
    /// Social is on but the active account has no launcher link.
    NotLinked,
    /// The last fetch failed.
    Failed,
    /// First fetch not back yet.
    Loading,
    Loaded,
}

/// Social gating first (disabled, then unlinked), then the list state.
pub(super) fn friends_kind(
    enabled: bool,
    has_token: bool,
    state: &social::FriendsListState,
) -> FriendsKind {
    use social::FriendsListState as S;
    if !enabled {
        FriendsKind::Disabled
    } else if !has_token {
        FriendsKind::NotLinked
    } else {
        match state {
            S::Failed(_) => FriendsKind::Failed,
            S::Unknown | S::Loading => FriendsKind::Loading,
            S::Loaded(_) => FriendsKind::Loaded,
        }
    }
}

/// The text of the friend-action toast: none while idle.
pub(super) fn friend_action_text(status: &social::FriendActionStatus) -> Option<String> {
    match status {
        social::FriendActionStatus::Idle => None,
        social::FriendActionStatus::Submitting => Some("sending…".to_string()),
        social::FriendActionStatus::Done(msg) | social::FriendActionStatus::Failed(msg) => {
            Some(msg.clone())
        }
    }
}

/// The toast's `(age, remaining)` seconds. Submitting has no landing time:
/// fully visible, no entrance, no expiry.
pub(super) fn toast_timing(age: Option<f32>) -> (f32, f32) {
    (
        age.unwrap_or(f32::INFINITY),
        age.map_or(f32::INFINITY, |a| {
            (social::FRIEND_ACTION_TOAST_S - a).max(0.0)
        }),
    )
}

/// An owned copy of what the Friends screen shows this frame (the renderer
/// borrows slices of it).
pub(super) struct FriendsFrame {
    kind: FriendsKind,
    friends: Vec<FriendRowView>,
    incoming: Vec<FriendRowView>,
    outgoing: Vec<FriendRowView>,
    error: Option<String>,
    toast_text: Option<String>,
    toast_failed: bool,
    toast_age: f32,
    toast_remaining: f32,
}

impl FriendsFrame {
    /// Rows exist only for a linked account; a request only ever lands a
    /// toast status through the gated call path, so with social disabled or
    /// unlinked there is never one.
    pub(super) fn capture(social: &social::SocialState, has_token: bool, time: f32) -> Self {
        let state = social.friends();
        let kind = friends_kind(social.enabled(), has_token, state);
        let (friends, incoming, outgoing, error) = match (has_token, state) {
            (true, social::FriendsListState::Loaded(list)) => (
                list.friends.iter().map(friend_entry_to_view).collect(),
                list.incoming.iter().map(friend_entry_to_view).collect(),
                list.outgoing.iter().map(friend_entry_to_view).collect(),
                None,
            ),
            (true, social::FriendsListState::Failed(msg)) => {
                (vec![], vec![], vec![], Some(msg.clone()))
            }
            _ => (vec![], vec![], vec![], None),
        };
        let (toast_age, toast_remaining) = toast_timing(social.friend_action_age(time));
        Self {
            kind,
            friends,
            incoming,
            outgoing,
            error,
            toast_text: friend_action_text(social.friend_action()),
            toast_failed: matches!(
                social.friend_action(),
                social::FriendActionStatus::Failed(_)
            ),
            toast_age,
            toast_remaining,
        }
    }

    pub(super) fn toast(&self) -> Option<screens::FriendActionToast<'_>> {
        self.toast_text
            .as_deref()
            .map(|text| screens::FriendActionToast {
                text,
                failed: self.toast_failed,
                age: self.toast_age,
                remaining: self.toast_remaining,
            })
    }

    pub(super) fn view<'a>(
        &'a self,
        toast: Option<&'a screens::FriendActionToast<'a>>,
    ) -> FriendsViewState<'a> {
        match self.kind {
            FriendsKind::Disabled => FriendsViewState::Disabled,
            FriendsKind::NotLinked => FriendsViewState::NotLinked,
            FriendsKind::Failed => FriendsViewState::Failed(self.error.as_deref().unwrap_or("")),
            FriendsKind::Loading => FriendsViewState::Loading,
            FriendsKind::Loaded => FriendsViewState::Loaded {
                friends: &self.friends,
                incoming: &self.incoming,
                outgoing: &self.outgoing,
                action: toast,
            },
        }
    }
}

pub(super) fn link_redeem_view(status: &social::LinkRedeemStatus) -> LinkRedeemView<'_> {
    match status {
        social::LinkRedeemStatus::Submitting => LinkRedeemView::Submitting,
        social::LinkRedeemStatus::Failed(m) => LinkRedeemView::Failed(m),
        _ => LinkRedeemView::Idle,
    }
}

/// One Account-tab row per stored account; the active one is flagged.
pub(super) fn account_rows(auth: &AuthService) -> Vec<AccountRowView<'_>> {
    let active = auth.active().map(|a| a.uuid.as_str());
    auth.accounts()
        .iter()
        .map(|a| AccountRowView {
            name: &a.name,
            uuid: &a.uuid,
            active: active == Some(a.uuid.as_str()),
        })
        .collect()
}

/// The user-facing text for a failed auth operation.
pub(super) fn auth_failure_text(op: &AuthOp) -> Option<String> {
    if let AuthOp::Failed(err) = op {
        Some(format_auth_error(err))
    } else {
        None
    }
}

pub(super) fn account_op_view<'a>(op: &'a AuthOp, failure: Option<&'a str>) -> AccountOpView<'a> {
    match op {
        AuthOp::Idle => AccountOpView::Idle,
        AuthOp::Working(stage) => AccountOpView::Working { stage },
        AuthOp::Failed(_) => AccountOpView::Failed {
            message: failure.unwrap_or("auth failed"),
        },
    }
}

pub(super) fn profile_rows<'a>(names: &'a [String], active: &str) -> Vec<ProfileRowView<'a>> {
    names
        .iter()
        .map(|n| ProfileRowView {
            name: n,
            active: n == active,
        })
        .collect()
}

/// The chord label for every registry action, resolved against the active
/// profile's bindings (default when unbound).
pub(super) fn keybind_chord_labels(keybinds: &BTreeMap<String, keybind::KeyChord>) -> Vec<String> {
    keybind::REGISTRY
        .iter()
        .map(|a| keybinds.get(a.id).copied().unwrap_or(a.default).label())
        .collect()
}

pub(super) fn keybind_rows<'a>(
    labels: &'a [String],
    capturing: Option<&str>,
) -> Vec<KeybindRowView<'a>> {
    keybind::REGISTRY
        .iter()
        .zip(labels)
        .map(|(a, label)| KeybindRowView {
            action_label: a.label,
            module: a.module,
            chord_label: label,
            capturing: capturing == Some(a.id),
        })
        .collect()
}

pub(super) fn server_widget_view(
    status: Option<&social::ServerStatus>,
    hovered: bool,
    hidden: bool,
    joinable: bool,
) -> screens::ServerWidgetView<'_> {
    screens::ServerWidgetView {
        data: status.map(|s| screens::ServerWidgetData {
            online: s.online,
            online_count: s.online_count,
            max_players: s.max_players,
            tps: &s.tps,
            players: &s.players,
        }),
        hovered,
        hidden,
        joinable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social::{FriendsList, FriendsListState as S};

    #[test]
    fn friends_kind_gates_on_social_then_link_then_list_state() {
        let loaded = S::Loaded(FriendsList::default());
        // Disabled beats everything.
        assert_eq!(friends_kind(false, true, &loaded), FriendsKind::Disabled);
        assert_eq!(
            friends_kind(false, false, &S::Unknown),
            FriendsKind::Disabled
        );
        // Enabled but unlinked: NotLinked whatever the cached list says.
        for st in [&S::Unknown, &loaded, &S::Failed("x".into())] {
            assert_eq!(friends_kind(true, false, st), FriendsKind::NotLinked);
        }
        assert_eq!(friends_kind(true, true, &S::Unknown), FriendsKind::Loading);
        assert_eq!(friends_kind(true, true, &S::Loading), FriendsKind::Loading);
        assert_eq!(
            friends_kind(true, true, &S::Failed("x".into())),
            FriendsKind::Failed
        );
        assert_eq!(friends_kind(true, true, &loaded), FriendsKind::Loaded);
    }

    #[test]
    fn toast_text_follows_the_action_status() {
        use social::FriendActionStatus as A;
        assert_eq!(friend_action_text(&A::Idle), None);
        assert_eq!(
            friend_action_text(&A::Submitting).as_deref(),
            Some("sending…")
        );
        assert_eq!(
            friend_action_text(&A::Done("Request sent".into())).as_deref(),
            Some("Request sent")
        );
        assert_eq!(
            friend_action_text(&A::Failed("nope".into())).as_deref(),
            Some("nope")
        );
    }

    #[test]
    fn toast_timing_is_open_ended_while_submitting_and_counts_down_after() {
        let (age, remaining) = toast_timing(None);
        assert!(age.is_infinite() && remaining.is_infinite());
        let (age, remaining) = toast_timing(Some(1.0));
        assert_eq!(age, 1.0);
        assert_eq!(remaining, social::FRIEND_ACTION_TOAST_S - 1.0);
        let (_, remaining) = toast_timing(Some(social::FRIEND_ACTION_TOAST_S + 5.0));
        assert_eq!(remaining, 0.0, "never negative");
    }

    #[test]
    fn account_op_view_maps_each_state() {
        assert!(matches!(
            account_op_view(&AuthOp::Idle, None),
            AccountOpView::Idle
        ));
        assert!(matches!(
            account_op_view(&AuthOp::Working("verifying"), None),
            AccountOpView::Working { stage: "verifying" }
        ));
        let failed = AuthOp::Failed(auth::AuthError::UserCancelled);
        let text = auth_failure_text(&failed);
        assert_eq!(text.as_deref(), Some("sign-in was cancelled."));
        assert!(matches!(
            account_op_view(&failed, text.as_deref()),
            AccountOpView::Failed {
                message: "sign-in was cancelled."
            }
        ));
        assert!(matches!(
            account_op_view(&failed, None),
            AccountOpView::Failed {
                message: "auth failed"
            }
        ));
        assert_eq!(auth_failure_text(&AuthOp::Idle), None);
    }

    #[test]
    fn the_active_profile_is_flagged() {
        let names = vec!["a".to_string(), "b".to_string()];
        let rows = profile_rows(&names, "b");
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].active && rows[1].active);
        assert!(profile_rows(&names, "zzz").iter().all(|r| !r.active));
    }

    #[test]
    fn keybind_rows_pair_labels_with_the_registry_and_flag_the_capture() {
        let mut binds = BTreeMap::new();
        let first = &keybind::REGISTRY[0];
        let labels = keybind_chord_labels(&binds);
        assert_eq!(labels.len(), keybind::REGISTRY.len());
        assert_eq!(labels[0], first.default.label(), "unbound → default");

        // A rebind shows up in the label.
        let other = keybind::REGISTRY
            .iter()
            .map(|a| a.default)
            .find(|d| *d != first.default)
            .expect("registry has distinct defaults");
        binds.insert(first.id.to_string(), other);
        assert_eq!(keybind_chord_labels(&binds)[0], other.label());

        let rows = keybind_rows(&labels, Some(first.id));
        assert_eq!(rows.len(), keybind::REGISTRY.len());
        assert!(rows[0].capturing);
        assert!(rows[1..].iter().all(|r| !r.capturing));
        assert_eq!(rows[0].action_label, first.label);
        assert!(keybind_rows(&labels, None).iter().all(|r| !r.capturing));
    }

    #[test]
    fn the_server_widget_view_copies_status_and_flags() {
        let status = social::ServerStatus {
            online: true,
            online_count: 3,
            max_players: 20,
            tps: "19.9".into(),
            players: vec!["a".into(), "b".into()],
        };
        let v = server_widget_view(Some(&status), true, false, true);
        let d = v.data.expect("data");
        assert!(d.online && d.online_count == 3 && d.max_players == 20);
        assert_eq!(d.tps, "19.9");
        assert_eq!(d.players.len(), 2);
        assert!(v.hovered && !v.hidden && v.joinable);
        assert!(server_widget_view(None, false, true, false).data.is_none());
    }

    #[test]
    fn link_redeem_view_maps_each_status() {
        use social::LinkRedeemStatus as L;
        assert!(matches!(link_redeem_view(&L::Idle), LinkRedeemView::Idle));
        assert!(matches!(
            link_redeem_view(&L::Submitting),
            LinkRedeemView::Submitting
        ));
        assert!(matches!(
            link_redeem_view(&L::Failed("bad code".into())),
            LinkRedeemView::Failed("bad code")
        ));
        // A success is harvested elsewhere; the modal shows idle meanwhile.
        assert!(matches!(
            link_redeem_view(&L::Success {
                token: "t".into(),
                discord_id: "d".into()
            }),
            LinkRedeemView::Idle
        ));
    }
}
