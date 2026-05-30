//! Social / chickenedin integration — Phase H.
//!
//! Owns the launcher's calls to the chickenedin bot API. H1 does one
//! thing: ask the bot whether each signed-in account's MC UUID is linked
//! to a Discord account (`GET /api/links/by-uuid`). The bot returns
//! `{"linked": bool}`; we cache the result per-UUID and the Settings →
//! Account tab surfaces it so the user knows whether they've run the
//! in-game `/link` flow for that account.
//!
//! Future H-steps add: per-user social token (H2), presence heartbeat
//! (H3), friend graph (H4–H5), Roblox-style join (H6). They will all
//! live in submodules under this one.
//!
//! Threading model mirrors [`crate::auth::service`]: one worker thread
//! per probe, results flow back over `mpsc`, the UI polls each frame.
//! No tokio/smol per the CLAUDE.md non-negotiables.
//!
//! Offline-first: nothing in this module runs until the launcher has at
//! least one signed-in MS account to probe. A signed-out launcher stays
//! fully offline. A probe failure is non-fatal — the UI renders it as
//! "unknown" rather than nagging.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Default bot API base. Override via env var `EWO_BOT_API_BASE` for
/// dev (e.g. point at a local bot during local iteration).
pub const DEFAULT_BOT_API_BASE: &str = "https://chickenedin.com/bot";

/// HTTP timeout for social-API calls. The endpoints are tiny JSON
/// (`{"linked": false}` etc.) — anything past 6s is a real failure,
/// not slowness.
const HTTP_TIMEOUT: Duration = Duration::from_secs(6);

/// Phase H3 — wall-time seconds between presence heartbeats. Bot
/// considers a presence row stale after 60s, so 30s gives one missed
/// heartbeat of slack before friends see us go offline.
const HEARTBEAT_INTERVAL_S: f32 = 30.0;

/// Phase H5 — refresh the friends list at most this often (seconds).
/// User actions (accept/decline/remove/add) trigger an immediate
/// refetch; this is the idle cadence.
const FRIENDS_REFRESH_INTERVAL_S: f32 = 30.0;

/// Phase H6 — poll the live network status (`/api/server-status`) at most
/// this often (seconds). Only ticked while the user is on the main menu.
const SERVER_STATUS_REFRESH_INTERVAL_S: f32 = 15.0;

/// The lobby the main-menu server widget joins on click. Host:port; the
/// client defaults the port to 25565 when omitted.
pub const CHICKENEDIN_LOBBY_ADDR: &str = "play.chickenedin.com";

fn bot_api_base() -> String {
    std::env::var("EWO_BOT_API_BASE")
        .unwrap_or_else(|_| DEFAULT_BOT_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn ureq_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("EwoClient/0.1 (+https://github.com/valtterisaarinen/ewoclient)")
        .build()
}

/// Whether a particular MC UUID is linked to a Discord account on
/// chickenedin. State machine: `Unknown` (initial) → `Probing` (in
/// flight) → terminal state (`Linked` / `NotLinked` / `Failed`).
#[derive(Clone, Debug)]
pub enum LinkStatus {
    /// Not probed yet.
    Unknown,
    /// Probe in flight.
    Probing,
    /// Bot confirmed this UUID has a `links` row.
    Linked,
    /// Bot confirmed this UUID has no `links` row.
    NotLinked,
    /// Probe failed — network down, bot down, JSON malformed, etc.
    /// The string is a short developer-facing reason for the log; the
    /// UI renders this like "unknown" without surfacing the detail.
    Failed(String),
}

/// Phase H2: state of the in-flight launcher-link code redemption.
/// The user enters a 6-digit code in the link modal; the launcher POSTs
/// it to `/api/launcher/link`; the bot returns a `social_token` we then
/// hand off to `AuthService::set_social_token`.
#[derive(Clone, Debug)]
pub enum LinkRedeemStatus {
    /// No redemption in flight; the modal is just an input field.
    Idle,
    /// POST in flight.
    Submitting,
    /// Bot returned a token. App must consume this (persist + clear).
    Success { token: String, discord_id: String },
    /// Redemption failed. String is a user-facing short message.
    Failed(String),
}

/// Phase H5: cached friends list state. Refreshed by polling
/// `/api/friends` on a 30s cadence (or eagerly after a mutation).
#[derive(Clone, Debug)]
pub enum FriendsListState {
    /// Never fetched (no social_token, or never on the Friends screen).
    Unknown,
    /// First fetch in flight.
    Loading,
    /// Fetched successfully. Subsequent refreshes don't transition
    /// back to `Loading` — they atomically swap the contents.
    Loaded(FriendsList),
    /// Most recent fetch errored. UI shows the short message.
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct FriendsList {
    pub friends: Vec<FriendEntry>,
    pub incoming: Vec<FriendEntry>,
    pub outgoing: Vec<FriendEntry>,
}

#[derive(Clone, Debug)]
pub struct FriendEntry {
    pub discord_id: String,
    pub minecraft_uuid: Option<String>,
    pub presence: Option<FriendPresence>,
}

#[derive(Clone, Debug)]
pub struct FriendPresence {
    pub location: String,
    pub server_addr: Option<String>,
    pub screen: Option<String>,
}

/// Phase H6 — a snapshot of the chickenedin network's live status, polled
/// from the public `/api/server-status`. Drives the main-menu server widget.
#[derive(Clone, Debug, Default)]
pub struct ServerStatus {
    /// `false` when the network is down / the status endpoint reported
    /// `online: false`. The widget renders "offline" in that case.
    pub online: bool,
    pub online_count: u32,
    pub max_players: u32,
    /// Usernames currently online (already public on the website). The
    /// widget shows up to a handful as an avatar/name strip.
    pub players: Vec<String>,
    /// TPS as the bot reports it — a free-form string like "19.8" or "N/A".
    pub tps: String,
}

/// State of an in-flight friend-mutation (request / respond / remove).
/// Drives the toast/inline status the Friends screen shows for the
/// most recent action. Cleared on next successful refresh or by the
/// caller after rendering.
#[derive(Clone, Debug)]
pub enum FriendActionStatus {
    Idle,
    Submitting,
    Done(String), // user-facing message ("Request sent", "Removed", etc.)
    Failed(String),
}

/// All social state the launcher holds. Owned by `App`.
pub struct SocialState {
    bot_api_base: String,
    /// MC UUID → link status. UUIDs are stored undashed, matching the
    /// shape `auth::MinecraftAccount.uuid` uses.
    statuses: HashMap<String, LinkStatus>,
    /// In-flight launcher-link redemption (H2). One at a time.
    link_redeem: LinkRedeemStatus,
    /// Phase H3 heartbeat state.
    heartbeat: HeartbeatTracker,
    /// Phase H5 friends-list cache + polling state.
    friends: FriendsListState,
    /// Wall-time of the most recent friends-list fetch (start, not
    /// completion). Drives the 30s refresh cadence.
    friends_last_fetch: Option<f32>,
    /// True while a friends-list GET is in flight.
    friends_in_flight: bool,
    /// Last mutation status (request/respond/remove) for the UI toast.
    friend_action: FriendActionStatus,
    /// Phase H6 — latest network status snapshot (or `None` until the first
    /// successful poll). A failed poll leaves the previous snapshot in place
    /// rather than blanking the widget.
    server_status: Option<ServerStatus>,
    server_status_last_fetch: Option<f32>,
    server_status_in_flight: bool,
    rx: Receiver<SocialEvent>,
    tx: Sender<SocialEvent>,
}

#[derive(Debug, Default)]
struct HeartbeatTracker {
    /// Wall-time of the most recent heartbeat we *started*. We rate-gate
    /// by start time rather than completion to avoid heartbeat storms
    /// when the bot is slow.
    last_sent: Option<f32>,
    /// `true` while a heartbeat worker is alive. Prevents two concurrent
    /// POSTs from the same launcher.
    in_flight: bool,
}

#[derive(Debug)]
enum SocialEvent {
    LinkProbeResult { uuid: String, status: LinkStatus },
    LinkRedeemResult(LinkRedeemStatus),
    HeartbeatDone,
    FriendsRefreshed(Result<FriendsList, String>),
    FriendActionDone(FriendActionStatus),
    ServerStatusResult(Option<ServerStatus>),
}

impl SocialState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            bot_api_base: bot_api_base(),
            statuses: HashMap::new(),
            link_redeem: LinkRedeemStatus::Idle,
            heartbeat: HeartbeatTracker::default(),
            friends: FriendsListState::Unknown,
            friends_last_fetch: None,
            friends_in_flight: false,
            friend_action: FriendActionStatus::Idle,
            server_status: None,
            server_status_last_fetch: None,
            server_status_in_flight: false,
            rx,
            tx,
        }
    }

    /// Current link status for `uuid`. `Unknown` for any UUID we
    /// haven't probed yet (and never will, until [`ensure_probed`] is
    /// called for it).
    pub fn link_status(&self, uuid: &str) -> LinkStatus {
        self.statuses
            .get(uuid)
            .cloned()
            .unwrap_or(LinkStatus::Unknown)
    }

    /// Idempotent: probe `uuid` if its status is still `Unknown`.
    /// Already-Probing/Linked/NotLinked accounts are a no-op. A
    /// previously-`Failed` probe is also a no-op for H1 — we don't
    /// auto-retry to avoid hammering a flapping bot. A manual retry
    /// affordance can be added later if needed.
    pub fn ensure_probed(&mut self, uuid: &str) {
        match self.statuses.get(uuid) {
            None | Some(LinkStatus::Unknown) => {}
            _ => return,
        }
        self.statuses
            .insert(uuid.to_string(), LinkStatus::Probing);

        let tx = self.tx.clone();
        let base = self.bot_api_base.clone();
        let uuid_s = uuid.to_string();
        let _ = thread::Builder::new()
            .name("ewo-social-link-probe".into())
            .spawn(move || {
                let status = probe_one(&base, &uuid_s);
                let _ = tx.send(SocialEvent::LinkProbeResult {
                    uuid: uuid_s,
                    status,
                });
            });
    }

    /// Drain pending worker events. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                SocialEvent::LinkProbeResult { uuid, status } => {
                    log::info!("social: link probe {} -> {:?}", uuid, status);
                    self.statuses.insert(uuid, status);
                }
                SocialEvent::LinkRedeemResult(status) => {
                    log::info!("social: link redeem -> {:?}", status);
                    self.link_redeem = status;
                }
                SocialEvent::HeartbeatDone => {
                    self.heartbeat.in_flight = false;
                }
                SocialEvent::FriendsRefreshed(result) => {
                    self.friends_in_flight = false;
                    match result {
                        Ok(list) => {
                            log::info!(
                                "social: friends refreshed — {} friends, {} incoming, {} outgoing",
                                list.friends.len(),
                                list.incoming.len(),
                                list.outgoing.len(),
                            );
                            self.friends = FriendsListState::Loaded(list);
                        }
                        Err(msg) => {
                            log::warn!("social: friends refresh failed: {}", msg);
                            self.friends = FriendsListState::Failed(msg);
                        }
                    }
                }
                SocialEvent::FriendActionDone(status) => {
                    log::info!("social: friend-action {:?}", status);
                    self.friend_action = status;
                }
                SocialEvent::ServerStatusResult(result) => {
                    self.server_status_in_flight = false;
                    // Keep the last good snapshot on failure rather than
                    // blanking the widget; only overwrite on success.
                    if let Some(status) = result {
                        self.server_status = Some(status);
                    }
                }
            }
        }
    }

    // ── Phase H2: launcher-link redemption ─────────────────────────────

    pub fn link_redeem(&self) -> &LinkRedeemStatus {
        &self.link_redeem
    }

    /// Fire a POST to `/api/launcher/link` with the user's typed code.
    /// No-op if a redemption is already in flight. Result lands in
    /// `link_redeem()` via the next `poll()` after the worker finishes.
    pub fn submit_link_code(&mut self, code: String) {
        if matches!(self.link_redeem, LinkRedeemStatus::Submitting) {
            return;
        }
        self.link_redeem = LinkRedeemStatus::Submitting;

        let tx = self.tx.clone();
        let base = self.bot_api_base.clone();
        let _ = thread::Builder::new()
            .name("ewo-social-link-redeem".into())
            .spawn(move || {
                let result = redeem_one(&base, &code);
                let _ = tx.send(SocialEvent::LinkRedeemResult(result));
            });
    }

    /// Reset the redemption state to `Idle`. Call after consuming a
    /// `Success` (persisting the token) or dismissing a `Failed` from
    /// the UI.
    pub fn clear_link_redeem(&mut self) {
        self.link_redeem = LinkRedeemStatus::Idle;
    }

    // ── Phase H3: presence heartbeat ───────────────────────────────────

    /// Per-frame entry point. Sends a heartbeat if at least
    /// `HEARTBEAT_INTERVAL_S` has elapsed since the last one started,
    /// and no in-flight POST is pending. Caller is responsible for
    /// passing a non-empty `social_token` — calling without one is a
    /// no-op (signed-out launchers stay fully offline).
    pub fn maybe_send_heartbeat(
        &mut self,
        time: f32,
        mc_uuid: &str,
        social_token: &str,
        location: HeartbeatLocation<'_>,
    ) {
        if social_token.is_empty() || mc_uuid.is_empty() {
            return;
        }
        if self.heartbeat.in_flight {
            return;
        }
        if let Some(last) = self.heartbeat.last_sent {
            if time - last < HEARTBEAT_INTERVAL_S {
                return;
            }
        }

        self.heartbeat.in_flight = true;
        self.heartbeat.last_sent = Some(time);

        // Snapshot all the borrowed data the worker needs.
        let base = self.bot_api_base.clone();
        let token = social_token.to_string();
        let mc_uuid_s = mc_uuid.to_string();
        let (loc, screen, server_addr) = match location {
            HeartbeatLocation::InLauncher { screen } => {
                ("launcher", Some(screen.to_string()), None)
            }
            HeartbeatLocation::InGame { server_addr } => {
                ("in_game", None, Some(server_addr.to_string()))
            }
        };
        let tx = self.tx.clone();

        let _ = thread::Builder::new()
            .name("ewo-social-heartbeat".into())
            .spawn(move || {
                send_heartbeat(
                    &base,
                    &token,
                    &mc_uuid_s,
                    loc,
                    screen.as_deref(),
                    server_addr.as_deref(),
                );
                let _ = tx.send(SocialEvent::HeartbeatDone);
            });
    }
}

/// Where the user is right now, from the launcher's POV. Maps onto the
/// `location` + `screen`/`server_addr` columns of the bot's `presence`
/// table.
#[derive(Copy, Clone, Debug)]
pub enum HeartbeatLocation<'a> {
    /// In the launcher itself; `screen` names the current Screen
    /// (`"main_menu"`, `"instances"`, `"settings"`, `"launching"`).
    InLauncher { screen: &'a str },
    /// Game is running. `server_addr` is the server we connected to,
    /// or a sentinel like `"singleplayer"`.
    InGame { server_addr: &'a str },
}

// ── Phase H5: friends list + mutations ────────────────────────────────

impl SocialState {
    pub fn friends(&self) -> &FriendsListState {
        &self.friends
    }

    pub fn friend_action(&self) -> &FriendActionStatus {
        &self.friend_action
    }

    pub fn clear_friend_action(&mut self) {
        self.friend_action = FriendActionStatus::Idle;
    }

    // ── Phase H6: live network status ──────────────────────────────────

    /// Latest network-status snapshot, or `None` before the first poll.
    pub fn server_status(&self) -> Option<&ServerStatus> {
        self.server_status.as_ref()
    }

    /// Poll `/api/server-status` (public, no token) if at least
    /// `SERVER_STATUS_REFRESH_INTERVAL_S` has elapsed since the last poll
    /// start. The caller gates this to the main-menu screen so we don't
    /// hammer the endpoint from screens that never show the widget.
    pub fn maybe_refresh_server_status(&mut self, time: f32) {
        if self.server_status_in_flight {
            return;
        }
        if let Some(last) = self.server_status_last_fetch {
            if time - last < SERVER_STATUS_REFRESH_INTERVAL_S {
                return;
            }
        }
        self.server_status_in_flight = true;
        self.server_status_last_fetch = Some(time);

        let tx = self.tx.clone();
        let base = self.bot_api_base.clone();
        let _ = thread::Builder::new()
            .name("ewo-social-server-status".into())
            .spawn(move || {
                let result = fetch_server_status(&base);
                let _ = tx.send(SocialEvent::ServerStatusResult(result));
            });
    }

    /// Refresh the friends list immediately, regardless of the 30s
    /// throttle. Call after a mutation succeeds, or when the user
    /// switches to the Friends screen.
    pub fn refresh_friends_now(&mut self, social_token: &str) {
        self.friends_last_fetch = None;
        self.maybe_refresh_friends(f32::INFINITY, social_token);
    }

    /// Refresh the friends list if at least `FRIENDS_REFRESH_INTERVAL_S`
    /// has passed since the last fetch start. No-op if `social_token`
    /// is empty (offline-first invariant) or a fetch is already running.
    pub fn maybe_refresh_friends(&mut self, time: f32, social_token: &str) {
        if social_token.is_empty() || self.friends_in_flight {
            return;
        }
        if let Some(last) = self.friends_last_fetch {
            if time != f32::INFINITY && time - last < FRIENDS_REFRESH_INTERVAL_S {
                return;
            }
        }

        // Promote Unknown → Loading on the very first fetch; later
        // refreshes leave the cached data visible until the new
        // payload arrives.
        if matches!(self.friends, FriendsListState::Unknown) {
            self.friends = FriendsListState::Loading;
        }

        self.friends_in_flight = true;
        if time != f32::INFINITY {
            self.friends_last_fetch = Some(time);
        }

        let tx = self.tx.clone();
        let base = self.bot_api_base.clone();
        let token = social_token.to_string();
        let _ = thread::Builder::new()
            .name("ewo-social-friends-refresh".into())
            .spawn(move || {
                let result = fetch_friends(&base, &token);
                let _ = tx.send(SocialEvent::FriendsRefreshed(result));
            });
    }

    pub fn submit_friend_request_by_name(&mut self, social_token: &str, mc_name: String) {
        self.dispatch_friend_action(
            social_token,
            FriendActionPayload::RequestByName(mc_name),
        );
    }

    pub fn respond_friend_request(
        &mut self,
        social_token: &str,
        from_discord_id: String,
        accept: bool,
    ) {
        self.dispatch_friend_action(
            social_token,
            FriendActionPayload::Respond {
                from_discord_id,
                accept,
            },
        );
    }

    pub fn remove_friend(&mut self, social_token: &str, discord_id: String) {
        self.dispatch_friend_action(
            social_token,
            FriendActionPayload::Remove(discord_id),
        );
    }

    fn dispatch_friend_action(
        &mut self,
        social_token: &str,
        payload: FriendActionPayload,
    ) {
        if social_token.is_empty() {
            return;
        }
        // Block overlapping mutations — sender of the inflight POST
        // gets the response next tick anyway.
        if matches!(self.friend_action, FriendActionStatus::Submitting) {
            return;
        }
        self.friend_action = FriendActionStatus::Submitting;
        // Pre-mark the friends list as in_flight so the per-frame
        // 30s tick doesn't kick its own GET while ours is in flight.
        self.friends_in_flight = true;
        let tx = self.tx.clone();
        let base = self.bot_api_base.clone();
        let token = social_token.to_string();
        let _ = thread::Builder::new()
            .name("ewo-social-friend-action".into())
            .spawn(move || {
                let status = run_friend_action(&base, &token, payload);
                let succeeded = matches!(status, FriendActionStatus::Done(_));
                let _ = tx.send(SocialEvent::FriendActionDone(status));
                // Chain a refresh so the UI reflects the new state.
                // On failure we still refresh — the server's view is
                // the source of truth, and the user's expectation is
                // that the list updates after they clicked something.
                let _ = succeeded;
                let refresh = fetch_friends(&base, &token);
                let _ = tx.send(SocialEvent::FriendsRefreshed(refresh));
            });
    }
}

#[derive(Debug)]
enum FriendActionPayload {
    RequestByName(String),
    Respond { from_discord_id: String, accept: bool },
    Remove(String),
}

impl Default for SocialState {
    fn default() -> Self {
        Self::new()
    }
}

/// MS-auth gives us undashed UUIDs (`d4f7c0...`). The bot's `links`
/// table stores them dashed (`d4f7c0-...-...`). Insert dashes so the
/// HTTP query matches what the DB sees.
fn uuid_with_dashes(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 32 {
        // Already dashed, or malformed — pass through unchanged.
        return s.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &cleaned[0..8],
        &cleaned[8..12],
        &cleaned[12..16],
        &cleaned[16..20],
        &cleaned[20..32],
    )
}

fn probe_one(base: &str, uuid: &str) -> LinkStatus {
    let dashed = uuid_with_dashes(uuid);
    let url = format!(
        "{}/api/links/by-uuid?minecraft_uuid={}",
        base,
        urlencoding::encode(&dashed),
    );

    let agent = ureq_agent();
    match agent.get(&url).call() {
        Ok(resp) => match resp.into_json::<serde_json::Value>() {
            Ok(json) => match json.get("linked").and_then(|v| v.as_bool()) {
                Some(true) => LinkStatus::Linked,
                Some(false) => LinkStatus::NotLinked,
                None => LinkStatus::Failed("response missing `linked` field".into()),
            },
            Err(e) => LinkStatus::Failed(format!("json parse: {}", e)),
        },
        Err(ureq::Error::Status(code, _)) => LinkStatus::Failed(format!("http {}", code)),
        Err(ureq::Error::Transport(e)) => LinkStatus::Failed(format!("transport: {}", e)),
    }
}

/// POST `/api/presence/heartbeat` with the user's social token. Failures
/// log once at warn level; we don't propagate them — the next 30s
/// tick retries. We never echo the bearer to any log.
fn send_heartbeat(
    base: &str,
    social_token: &str,
    mc_uuid: &str,
    location: &str,
    screen: Option<&str>,
    server_addr: Option<&str>,
) {
    let url = format!("{}/api/presence/heartbeat", base);
    let mut body = serde_json::json!({
        "minecraft_uuid": mc_uuid,
        "location": location,
        "visibility": "friends",
    });
    if let Some(s) = screen {
        body["screen"] = serde_json::Value::String(s.to_string());
    }
    if let Some(s) = server_addr {
        body["server_addr"] = serde_json::Value::String(s.to_string());
    }

    let agent = ureq_agent();
    let bearer = format!("Bearer {}", social_token);
    match agent.post(&url).set("Authorization", &bearer).send_json(body) {
        Ok(_) => {}
        Err(ureq::Error::Status(code, _)) => {
            log::warn!("social: heartbeat HTTP {}", code);
        }
        Err(ureq::Error::Transport(e)) => {
            log::warn!("social: heartbeat transport error: {}", e);
        }
    }
}

/// Phase H5: GET `/api/friends` and parse the response into a
/// `FriendsList`. Returns an Err with a short user-facing reason on
/// HTTP failure or malformed payload.
fn fetch_friends(base: &str, social_token: &str) -> Result<FriendsList, String> {
    let url = format!("{}/api/friends", base);
    let agent = ureq_agent();
    let bearer = format!("Bearer {}", social_token);
    let resp = match agent.get(&url).set("Authorization", &bearer).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            return Err("launcher unlinked (401)".into());
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("http {}", code));
        }
        Err(ureq::Error::Transport(e)) => {
            return Err(format!("transport: {}", e));
        }
    };

    let json: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("json parse: {}", e))?;

    fn parse_section(arr: &serde_json::Value) -> Vec<FriendEntry> {
        arr.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let discord_id =
                            item.get("discord_id")?.as_str()?.to_string();
                        let minecraft_uuid = item
                            .get("minecraft_uuid")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let presence = item.get("presence").and_then(|p| {
                            Some(FriendPresence {
                                location: p
                                    .get("location")?
                                    .as_str()?
                                    .to_string(),
                                server_addr: p
                                    .get("server_addr")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string),
                                screen: p
                                    .get("screen")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string),
                            })
                        });
                        Some(FriendEntry {
                            discord_id,
                            minecraft_uuid,
                            presence,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    Ok(FriendsList {
        friends: parse_section(&json["friends"]),
        incoming: parse_section(&json["incoming"]),
        outgoing: parse_section(&json["outgoing"]),
    })
}

/// Phase H6: GET the public `/api/server-status`. Returns `None` on any
/// failure (network down, bot down, malformed) — the caller keeps the last
/// good snapshot in that case. A successful fetch where the bot reports the
/// network as down comes back as `Some(ServerStatus { online: false, .. })`
/// so the widget can render the "offline" state distinctly.
fn fetch_server_status(base: &str) -> Option<ServerStatus> {
    let url = format!("{}/api/server-status", base);
    let agent = ureq_agent();
    let json: serde_json::Value = match agent.get(&url).call() {
        Ok(resp) => resp.into_json().ok()?,
        Err(e) => {
            log::warn!("social: server-status fetch failed: {}", e);
            return None;
        }
    };
    let players = json
        .get("players")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // TPS may arrive as a string ("19.8") or a number — accept both.
    let tps = match json.get("tps") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => "N/A".to_string(),
    };
    Some(ServerStatus {
        online: json.get("online").and_then(|v| v.as_bool()).unwrap_or(false),
        online_count: json
            .get("online_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        max_players: json
            .get("max_players")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        players,
        tps,
    })
}

/// Phase H5: dispatch a mutation (request / respond / remove) and map
/// the response to a `FriendActionStatus`. The Done message is the
/// toast the UI shows.
fn run_friend_action(
    base: &str,
    social_token: &str,
    payload: FriendActionPayload,
) -> FriendActionStatus {
    let agent = ureq_agent();
    let bearer = format!("Bearer {}", social_token);
    match payload {
        FriendActionPayload::RequestByName(name) => {
            let url = format!("{}/api/friends/request", base);
            let body = serde_json::json!({ "target_mc_name": name });
            match agent
                .post(&url)
                .set("Authorization", &bearer)
                .send_json(body)
            {
                Ok(resp) => {
                    let status = resp
                        .into_json::<serde_json::Value>()
                        .ok()
                        .and_then(|v| {
                            v.get("status").and_then(|s| s.as_str()).map(str::to_string)
                        })
                        .unwrap_or_else(|| "unknown".into());
                    let msg = match status.as_str() {
                        "pending" => format!("request sent to {}", name),
                        "accepted" => format!("you and {} are now friends", name),
                        "already_friends" => format!("{} is already your friend", name),
                        "already_requested" => format!("request to {} is pending", name),
                        "blocked" => format!("{} blocked", name),
                        "self" => "you can't friend yourself".into(),
                        other => format!("server replied: {}", other),
                    };
                    FriendActionStatus::Done(msg)
                }
                Err(ureq::Error::Status(404, resp)) => {
                    let body = resp
                        .into_json::<serde_json::Value>()
                        .ok()
                        .and_then(|v| {
                            v.get("status")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_else(|| "not-found".into());
                    FriendActionStatus::Failed(match body.as_str() {
                        "not-found" => format!("no Minecraft account named {}", name),
                        "not-linked" => {
                            format!("{} hasn't linked their Discord", name)
                        }
                        other => other.to_string(),
                    })
                }
                Err(ureq::Error::Status(code, _)) => {
                    FriendActionStatus::Failed(format!("http {}", code))
                }
                Err(ureq::Error::Transport(e)) => {
                    FriendActionStatus::Failed(format!("transport: {}", e))
                }
            }
        }
        FriendActionPayload::Respond {
            from_discord_id,
            accept,
        } => {
            let url = format!("{}/api/friends/respond", base);
            let body = serde_json::json!({
                "request_from_discord_id": from_discord_id.parse::<i64>().unwrap_or(0),
                "action": if accept { "accept" } else { "decline" },
            });
            match agent
                .post(&url)
                .set("Authorization", &bearer)
                .send_json(body)
            {
                Ok(_) => FriendActionStatus::Done(
                    if accept {
                        "request accepted".into()
                    } else {
                        "request declined".into()
                    },
                ),
                Err(ureq::Error::Status(404, _)) => {
                    FriendActionStatus::Failed("request expired".into())
                }
                Err(ureq::Error::Status(code, _)) => {
                    FriendActionStatus::Failed(format!("http {}", code))
                }
                Err(ureq::Error::Transport(e)) => {
                    FriendActionStatus::Failed(format!("transport: {}", e))
                }
            }
        }
        FriendActionPayload::Remove(discord_id) => {
            let url = format!("{}/api/friends/{}", base, discord_id);
            match agent
                .delete(&url)
                .set("Authorization", &bearer)
                .call()
            {
                Ok(_) => FriendActionStatus::Done("friend removed".into()),
                Err(ureq::Error::Status(404, _)) => {
                    FriendActionStatus::Failed("already removed".into())
                }
                Err(ureq::Error::Status(code, _)) => {
                    FriendActionStatus::Failed(format!("http {}", code))
                }
                Err(ureq::Error::Transport(e)) => {
                    FriendActionStatus::Failed(format!("transport: {}", e))
                }
            }
        }
    }
}

/// POST `/api/launcher/link` with the typed code; map the response to
/// a `LinkRedeemStatus`. Error messages are user-facing — they go into
/// the modal's status line directly.
fn redeem_one(base: &str, code: &str) -> LinkRedeemStatus {
    let url = format!("{}/api/launcher/link", base);
    let body = serde_json::json!({ "code": code });

    let agent = ureq_agent();
    match agent.post(&url).send_json(body) {
        Ok(resp) => match resp.into_json::<serde_json::Value>() {
            Ok(json) => {
                let token = json
                    .get("social_token")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let discord_id = json
                    .get("discord_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match (token, discord_id) {
                    (Some(t), Some(d)) => LinkRedeemStatus::Success {
                        token: t,
                        discord_id: d,
                    },
                    _ => LinkRedeemStatus::Failed(
                        "bot returned a malformed response".into(),
                    ),
                }
            }
            Err(e) => LinkRedeemStatus::Failed(format!("bad response: {}", e)),
        },
        Err(ureq::Error::Status(404, _)) => {
            LinkRedeemStatus::Failed("code expired or already used".into())
        }
        Err(ureq::Error::Status(429, _)) => {
            LinkRedeemStatus::Failed("rate limited — wait a minute".into())
        }
        Err(ureq::Error::Status(code, _)) => {
            LinkRedeemStatus::Failed(format!("server error ({})", code))
        }
        Err(ureq::Error::Transport(e)) => {
            LinkRedeemStatus::Failed(format!("could not reach the bot: {}", e))
        }
    }
}
