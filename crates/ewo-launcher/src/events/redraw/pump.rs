//! Per-frame service pumps: poll every background channel, apply the
//! results to launcher state, and settle finished downloads. Order matters
//! and is the order the original frame had.

use super::*;
use ewo_render::screens::instances::InstanceStatus;

/// The version id an instance's download is keyed by: the last
/// " · "-separated part of its meta line (`commit_new_instance` writes
/// "<LOADER> · <version>").
fn instance_version_id(version: &str) -> &str {
    version.rsplit(" · ").next().unwrap_or(version)
}

/// The list-row badge percentage for a download in flight, or `None` while
/// the total is unknown. Capped at 99 so a row never shows 100 % before the
/// instance flips to Ready.
fn download_percent(downloaded: u64, total: Option<u64>) -> Option<u32> {
    let total = total.filter(|&t| t > 0)?;
    Some(((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 99.0) as u32)
}

/// The per-profile `ewo-friends.txt` snapshot the in-game FRIENDS overlay
/// tab reads: one tab-separated line per accepted friend,
/// `<online 0|1>\t<name>\t<presence>\t<server_addr>`.
fn friends_snapshot(list: &social::FriendsList) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for e in &list.friends {
        let v = friend_entry_to_view(e);
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}",
            if v.online { 1 } else { 0 },
            v.display_name,
            v.presence,
            v.server_addr.unwrap_or_default(),
        );
    }
    s
}

/// Flip every not-yet-Ready instance whose version has a finished download
/// to Ready. Returns whether anything changed (so the caller persists).
fn mark_downloaded_ready(instances: &mut [Instance], completed_versions: &[String]) -> bool {
    let mut any_changed = false;
    for inst in instances.iter_mut() {
        if inst.status == InstanceStatus::Ready {
            continue;
        }
        let v = instance_version_id(&inst.version);
        if completed_versions.iter().any(|w| w == v) {
            inst.status = InstanceStatus::Ready;
            any_changed = true;
            log::info!("instances: \"{}\" → Ready (version {})", inst.name, v);
        }
    }
    any_changed
}

impl App {
    /// Poll everything that reports in the background and apply the results.
    pub(super) fn pump_services(&mut self, time: f32) {
        self.pump_accounts_and_social(time);
        self.pump_downloads();
        self.pump_launch_preparation(time);
        self.pump_runtime_events(time);
        self.pump_launch_events(time);
        self.settle_finished_downloads();
        self.sync_version_dropdown();
    }

    /// Auth worker results, then the social layer: worker results, the
    /// friends snapshot, the presence heartbeat, link-redemption harvest and
    /// the network-status poll.
    fn pump_accounts_and_social(&mut self, time: f32) {
        self.auth.poll();
        // Social: drain worker results. Every request below is gated
        // inside `SocialState` on a configured base URL plus the
        // active account's social_token (see `social::call_allowed`),
        // so signed-out and unlinked launchers make no social calls.
        self.social.poll(time);
        // FRIENDS overlay tab — when the friends list changes,
        // rewrite the per-profile `ewo-friends.txt` snapshot the
        // in-game cdylib reads. One tab-separated line per accepted
        // friend: <online 0|1>\t<name>\t<presence>\t<server_addr>.
        if self.social.take_friends_dirty() {
            if let social::FriendsListState::Loaded(list) = self.social.friends() {
                let s = friends_snapshot(list);
                if let Some(dir) = profile::active_dir() {
                    if let Err(e) = util::atomic_write(&dir.join("ewo-friends.txt"), s.as_bytes()) {
                        log::warn!("social: write ewo-friends.txt failed: {}", e);
                    }
                }
            }
        }
        // Phase H3: fire a presence heartbeat (rate-gated to 30s
        // inside `maybe_send_heartbeat`). Only when both an
        // active account AND a live social_token are available
        // — i.e. the user has completed the launcher-link flow.
        let heartbeat_inputs: Option<(String, String)> = self.auth.active().and_then(|a| {
            self.auth
                .social_token(&a.uuid)
                .map(|t| (a.uuid.clone(), t.to_string()))
        });
        if let Some((mc_uuid, token)) = heartbeat_inputs {
            // H6: while a server-join JVM is alive, advertise
            // `in_game · <addr>` so friends see (and can join) us.
            // `launch_rx` is `Some` only while the child runs; once
            // it exits we fall back to the current launcher screen.
            let location = match (self.launch_rx.is_some(), self.active_server.as_deref()) {
                (true, Some(addr)) => social::HeartbeatLocation::InGame { server_addr: addr },
                _ => social::HeartbeatLocation::InLauncher {
                    screen: screen_name(self.screen),
                },
            };
            self.social
                .maybe_send_heartbeat(time, &mc_uuid, &token, location);
            // Phase H5: refresh friends list on the 30s cadence.
            // Post-mutation refreshes are chained inside the
            // social worker thread (see `dispatch_friend_action`).
            self.social.maybe_refresh_friends(time, &token);
        }
        // Phase H2: harvest a successful launcher-link redemption
        // immediately — persist the token next to the MC account the
        // code was submitted for, clear the redemption state, close
        // the modal.
        let pending_token: Option<(String, String)> =
            if let social::LinkRedeemStatus::Success { token, .. } = self.social.link_redeem() {
                self.social
                    .link_redeem_uuid()
                    .map(|uuid| (uuid.to_string(), token.clone()))
            } else {
                None
            };
        if let Some((uuid, token)) = pending_token {
            self.auth.set_social_token(&uuid, token);
            self.social.clear_link_redeem();
            self.launcher_link_modal.close();
            // The fresh token is usable immediately: pull the friends
            // list now (rather than on the 30 s cadence) so the screen
            // has first content. Only for the active account, since the
            // list cache belongs to it.
            if self.auth.active().map(|a| a.uuid.as_str()) == Some(uuid.as_str()) {
                self.refresh_friends_on_arrival();
            }
        }
        // H6 — poll the network status only while it's on screen (the
        // main-menu widget) and only once the active account is linked.
        // 15s cadence inside the call.
        if self.screen == Screen::MainMenu {
            let token = self.active_social_token();
            self.social
                .maybe_refresh_server_status(time, token.as_deref());
        }
    }

    /// Version-manifest and download-service polls, then the per-instance
    /// download progress for the list-row badge.
    fn pump_downloads(&mut self) {
        self.versions.poll();
        self.downloads.poll();

        // Sync per-instance download progress for the list-row
        // badge. Only Pending instances are interesting; we
        // clear the map first so completed downloads stop
        // showing a stale percentage.
        self.instance_prefs.download_pct.clear();
        for inst in self.instances.iter() {
            if inst.status != InstanceStatus::Pending {
                continue;
            }
            let v = instance_version_id(&inst.version);
            if let Some(status) = self.downloads.status(v) {
                if let Some(pct) = download_percent(status.downloaded, status.total) {
                    self.instance_prefs
                        .download_pct
                        .insert(inst.name.clone(), pct);
                }
            }
        }
    }

    /// The launch pipeline's waiting states: token refresh, background
    /// preparation, JRE re-scan, Java-runtime dropdown labels.
    fn pump_launch_preparation(&mut self, time: f32) {
        // Continue a launch that was waiting on a token refresh.
        self.poll_auth_launch(time);
        // Finish a JVM launch whose background preparation is done.
        self.poll_prepare(time);
        // Retry a launch deferred on a JRE download once the background
        // re-scan that picks the new JRE up has finished.
        self.poll_jre_rescan(time);
        // Java-runtime dropdown labels from the warm-up scan.
        self.poll_jre_labels();
    }

    /// Flip any instances whose download job just finished from `Pending`
    /// to `Ready` and persist. We match on the version *string* of the
    /// most-recent job: the same instance can appear multiple times with
    /// different IDs, but the status flips per-instance.
    fn settle_finished_downloads(&mut self) {
        let mut completed_versions: Vec<String> = Vec::new();
        for (vid, status) in self.downloads.iter_statuses() {
            if status.done && status.error.is_none() {
                completed_versions.push(vid.clone());
            }
        }
        if !completed_versions.is_empty()
            && mark_downloaded_ready(&mut self.instances, &completed_versions)
        {
            persistence::save_instances(&self.instances);
        }
    }

    /// Sync the live version manifest into the new-instance modal's
    /// dropdown source.
    fn sync_version_dropdown(&mut self) {
        // Sync the live version manifest into the new-instance
        // modal's dropdown source. Filter to releases by default;
        // a "Show snapshots" toggle could later flip the second
        // arg. List goes from newest → oldest (Mojang's order).
        if let Some(manifest) = self.versions.manifest() {
            let want: Vec<String> = manifest
                .filtered_for_dropdown(false)
                .iter()
                .map(|e| e.id.clone())
                .collect();
            if want != self.modal.mc_versions {
                self.modal.apply_versions(want);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social::{FriendEntry, FriendPresence, FriendsList};

    fn inst(version: &str, status: InstanceStatus) -> Instance {
        let mut i = Instance::new("n".into(), version.into(), "never".into(), Vec::new());
        i.status = status;
        i
    }

    #[test]
    fn the_version_id_is_the_last_meta_segment() {
        assert_eq!(instance_version_id("FABRIC · 1.21.4"), "1.21.4");
        assert_eq!(instance_version_id("1.21.4"), "1.21.4");
        assert_eq!(instance_version_id("A · B · 26.2"), "26.2");
        assert_eq!(instance_version_id(""), "");
    }

    #[test]
    fn download_percent_needs_a_total_and_never_reaches_100() {
        assert_eq!(download_percent(10, None), None);
        assert_eq!(download_percent(10, Some(0)), None);
        assert_eq!(download_percent(0, Some(200)), Some(0));
        assert_eq!(download_percent(50, Some(200)), Some(25));
        assert_eq!(download_percent(200, Some(200)), Some(99));
        assert_eq!(
            download_percent(400, Some(200)),
            Some(99),
            "overshoot clamps"
        );
    }

    #[test]
    fn only_instances_with_a_finished_download_become_ready() {
        let mut list = vec![
            inst("VANILLA · 1.21", InstanceStatus::Pending),
            inst("VANILLA · 26.2", InstanceStatus::Pending),
            inst("1.21", InstanceStatus::Pending),
            inst("VANILLA · 1.21", InstanceStatus::Ready),
        ];
        let changed = mark_downloaded_ready(&mut list, &["1.21".to_string()]);
        assert!(changed);
        let st: Vec<_> = list.iter().map(|i| i.status).collect();
        assert_eq!(
            st,
            vec![
                InstanceStatus::Ready,
                InstanceStatus::Pending,
                InstanceStatus::Ready,
                InstanceStatus::Ready,
            ]
        );
        assert!(
            !mark_downloaded_ready(&mut list, &["1.21".to_string()]),
            "second pass changes nothing"
        );
        assert!(!mark_downloaded_ready(&mut list, &[]));
    }

    #[test]
    fn friends_snapshot_is_one_tab_separated_line_per_friend() {
        let mk = |uuid: &str, presence: Option<FriendPresence>| FriendEntry {
            discord_id: "d".into(),
            minecraft_uuid: Some(uuid.into()),
            presence,
        };
        let list = FriendsList {
            friends: vec![
                mk("00000000-0000-0000-0000-0000deadbeef", None),
                mk(
                    "00000000-0000-0000-0000-0000cafef00d",
                    Some(FriendPresence {
                        location: "in_game".into(),
                        server_addr: Some("play.example:25565".into()),
                        screen: None,
                    }),
                ),
            ],
            // Requests are not part of the overlay snapshot.
            incoming: vec![mk("00000000-0000-0000-0000-000011112222", None)],
            outgoing: Vec::new(),
        };
        assert_eq!(
            friends_snapshot(&list),
            "0\tPlayer deadbeef\toffline\t\n\
             1\tPlayer cafef00d\tin-game · play.example:25565\tplay.example:25565\n"
        );
        assert_eq!(friends_snapshot(&FriendsList::default()), "");
    }
}
