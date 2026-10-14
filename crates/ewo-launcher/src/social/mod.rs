//! Social / Frogsy integration — Phase H.
//!
//! Owns the launcher's calls to the Frogsy bot API (the network was called
//! "chickenedin" before its 2026 rename): launcher-link code redemption
//! (H2), presence heartbeat (H3), the friend graph (H4–H5) and the live
//! network status the main-menu widget shows (H6).
//!
//! Threading model mirrors [`crate::auth::service`]: one worker thread
//! per request, results flow back over `mpsc`, the UI polls each frame.
//! No tokio/smol per the CLAUDE.md non-negotiables.
//!
//! **Off unless configured.** There is no compiled-in host: the old default
//! (`chickenedin.com`) stopped resolving, and whoever registers a dead
//! default domain receives every linked user's bearer token, UUID and
//! presence, and controls the server addresses the launcher offers to
//! join. The base URL comes only from `EWO_BOT_API_BASE`, must be
//! `https://`, and anything else disables social with a warning
//! ([`config_from`]).
//!
//! **Only once linked.** Even when configured, every call except the
//! launcher-link redemption requires the active account's `social_token`
//! ([`call_allowed`]). Signed out, or signed in but unlinked, the launcher
//! makes no social calls at all. Redemption is the one exception because
//! it is how a token is obtained, and it runs only when the user submits
//! a code in the launcher-link modal.
//!
//! The H1 link probe (`GET /api/links/by-uuid` for every signed-in
//! account) is gone. It only chose the wording of the Settings line, and
//! it sent each signed-in account's UUID to the bot before the user had
//! opted in to anything. Linking doesn't need it: the bot mints a
//! `/launcher-link` code only for a Discord-linked account, and
//! redemption returns the `discord_id`, so holding a `social_token` is
//! itself the proof of a link. The Settings line now derives from the
//! token alone.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Env var naming the bot API base (e.g. `https://example.net/bot`).
/// Unset or empty = social disabled.
pub const BOT_API_BASE_ENV: &str = "EWO_BOT_API_BASE";

/// Env var naming the lobby (`host[:port]`) the main-menu network widget
/// joins on click. Unset = the widget shows status but isn't joinable.
pub const LOBBY_ADDR_ENV: &str = "EWO_LOBBY_ADDR";

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

/// How long a settled friend-action toast (Done / Failed) stays visible
/// next to the Friends list before clearing itself.
pub const FRIEND_ACTION_TOAST_S: f32 = 6.0;

/// Phase H6 — poll the live network status (`/api/server-status`) at most
/// this often (seconds). Only ticked while the user is on the main menu.
const SERVER_STATUS_REFRESH_INTERVAL_S: f32 = 15.0;

/// Where the social API lives, once the user has configured one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocialConfig {
    /// Validated `https://` base, no trailing slash.
    pub base: String,
    /// Lobby (`host[:port]`, the client defaults the port to 25565) the
    /// network widget joins on click, if configured. There is no default:
    /// the old `play.chickenedin.com` no longer resolves.
    pub lobby_addr: Option<String>,
}

/// Validate a bot API base URL. Only `https://<host>[/path]` passes: the
/// launcher sends bearer tokens to this host and joins server addresses
/// it returns, so plaintext `http://` (or any other scheme) is refused.
/// Returns the normalised base (lower-case scheme, no trailing slash).
pub fn validate_base(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("contains whitespace".into());
    }
    const SCHEME: &str = "https://";
    if !s
        .get(..SCHEME.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(SCHEME))
    {
        return Err("must start with https://".into());
    }
    let rest = &s[SCHEME.len()..];
    if rest.contains(['?', '#']) {
        return Err("must not carry a query or fragment".into());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return Err("must not carry credentials".into());
    }
    if authority.is_empty() || authority.starts_with(':') {
        return Err("has no host".into());
    }
    Ok(format!("{SCHEME}{}", rest.trim_end_matches('/')))
}

/// Build the social config from the raw env values. `None` means social
/// is disabled: the base is unset, empty, or invalid (an invalid value
/// logs a warning).
pub fn config_from(base_raw: Option<&str>, lobby_raw: Option<&str>) -> Option<SocialConfig> {
    let raw = base_raw.map(str::trim).filter(|s| !s.is_empty())?;
    let base = match validate_base(raw) {
        Ok(b) => b,
        Err(why) => {
            log::warn!("social: ignoring {BOT_API_BASE_ENV} ({why}); social features disabled");
            return None;
        }
    };
    let lobby_addr = lobby_raw
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c.is_control()))
        .map(str::to_string);
    Some(SocialConfig { base, lobby_addr })
}

fn config_from_env() -> Option<SocialConfig> {
    let base = std::env::var(BOT_API_BASE_ENV).ok();
    let lobby = std::env::var(LOBBY_ADDR_ENV).ok();
    let cfg = config_from(base.as_deref(), lobby.as_deref());
    match &cfg {
        Some(c) => log::info!("social: enabled, bot API at {}", c.base),
        None => log::info!("social: not configured, no social calls will be made"),
    }
    cfg
}

/// The kinds of request this module issues, for [`call_allowed`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SocialCall {
    /// `POST /api/launcher/link`: redeem a code the user typed.
    LinkRedeem,
    /// `GET /api/server-status` (public, but still only once linked).
    ServerStatus,
    /// `POST /api/presence/heartbeat`.
    Heartbeat,
    /// `GET /api/friends`.
    Friends,
    /// `POST /api/friends/request|respond`, `DELETE /api/friends/{id}`.
    FriendAction,
}

/// The single gate every social request passes. Nothing goes out unless a
/// base URL is configured; beyond that, only the user-initiated link
/// redemption runs without the active account's `social_token`.
pub fn call_allowed(configured: bool, social_token: Option<&str>, call: SocialCall) -> bool {
    if !configured {
        return false;
    }
    match call {
        SocialCall::LinkRedeem => true,
        SocialCall::ServerStatus
        | SocialCall::Heartbeat
        | SocialCall::Friends
        | SocialCall::FriendAction => social_token.is_some_and(|t| !t.is_empty()),
    }
}

/// `DELETE /api/friends/{discord_id}`. The id comes from the bot's JSON,
/// so it is percent-encoded rather than trusted to be digits.
fn remove_friend_url(base: &str, discord_id: &str) -> String {
    format!("{}/api/friends/{}", base, urlencoding::encode(discord_id))
}

fn ureq_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build()
}

/// Phase H2: state of the in-flight launcher-link code redemption.
/// The user enters a 6-digit code in the link modal; the launcher POSTs
/// it to `/api/launcher/link`; the bot returns a `social_token` we then
/// hand off to `AuthService::set_social_token`.
#[derive(Clone)]
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

/// Hand-written so the social token never reaches a log via `{:?}`.
impl std::fmt::Debug for LinkRedeemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("Idle"),
            Self::Submitting => f.write_str("Submitting"),
            Self::Success { discord_id, .. } => f
                .debug_struct("Success")
                .field("token", &"<redacted>")
                .field("discord_id", discord_id)
                .finish(),
            Self::Failed(m) => f.debug_tuple("Failed").field(m).finish(),
        }
    }
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

/// Phase H6 — a snapshot of the Frogsy network's live status, polled
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
    /// `None` = social disabled: every request method is a no-op.
    config: Option<SocialConfig>,
    /// In-flight launcher-link redemption (H2). One at a time.
    link_redeem: LinkRedeemStatus,
    /// The account (undashed MC UUID) the in-flight redemption is for, so
    /// a token that arrives after an account switch lands on the account
    /// the user linked, not whichever is active at harvest time.
    link_redeem_uuid: Option<String>,
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
    /// Wall-time the current settled `friend_action` landed. Drives the
    /// toast auto-expire (a settled action message shouldn't sit next to
    /// the list forever) and its fade.
    friend_action_at: Option<f32>,
    /// Phase H6 — latest network status snapshot (or `None` until the first
    /// successful poll). A failed poll leaves the previous snapshot in place
    /// rather than blanking the widget.
    server_status: Option<ServerStatus>,
    server_status_last_fetch: Option<f32>,
    server_status_in_flight: bool,
    /// H6/FRIENDS-tab — set when the friends list changes; the main loop
    /// drains it to rewrite `<profile>/ewo-friends.txt` for the in-game
    /// FRIENDS overlay tab. A take-once flag.
    friends_snapshot_dirty: bool,
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
    LinkRedeemResult(LinkRedeemStatus),
    HeartbeatDone,
    FriendsRefreshed(Result<FriendsList, String>),
    FriendActionDone(FriendActionStatus),
    ServerStatusResult(Option<ServerStatus>),
}

impl SocialState {
    /// Read the config from the environment (see [`config_from`]).
    pub fn new() -> Self {
        Self::with_config(config_from_env())
    }

    pub fn with_config(config: Option<SocialConfig>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            config,
            link_redeem: LinkRedeemStatus::Idle,
            link_redeem_uuid: None,
            heartbeat: HeartbeatTracker::default(),
            friends: FriendsListState::Unknown,
            friends_last_fetch: None,
            friends_in_flight: false,
            friend_action: FriendActionStatus::Idle,
            friend_action_at: None,
            server_status: None,
            server_status_last_fetch: None,
            server_status_in_flight: false,
            friends_snapshot_dirty: false,
            rx,
            tx,
        }
    }

    /// Take-once: `true` exactly once after each friends-list change. The
    /// caller rewrites the in-game `ewo-friends.txt` snapshot when set.
    pub fn take_friends_dirty(&mut self) -> bool {
        let d = self.friends_snapshot_dirty;
        self.friends_snapshot_dirty = false;
        d
    }

    /// Whether a social API is configured at all. `false` = the launcher
    /// makes no social calls and the UI hides or neutralises social
    /// surfaces.
    pub fn enabled(&self) -> bool {
        self.config.is_some()
    }

    /// The configured lobby address the network widget joins, if any.
    pub fn lobby_addr(&self) -> Option<&str> {
        self.config.as_ref()?.lobby_addr.as_deref()
    }

    /// The base URL, if `call` may run with this token (see
    /// [`call_allowed`]). Every request method goes through this.
    fn base_for(&self, social_token: Option<&str>, call: SocialCall) -> Option<String> {
        let cfg = self.config.as_ref()?;
        call_allowed(true, social_token, call).then(|| cfg.base.clone())
    }

    /// Drain pending worker events. Call once per frame with the current
    /// wall-time (used for the friend-action toast's auto-expire).
    pub fn poll(&mut self, now: f32) {
        // A Done/Failed action message clears itself after a few seconds
        // so the Friends screen doesn't accumulate stale statuses.
        // In-flight actions are never expired.
        if let Some(at) = self.friend_action_at {
            if now - at >= FRIEND_ACTION_TOAST_S
                && !matches!(self.friend_action, FriendActionStatus::Submitting)
            {
                self.friend_action = FriendActionStatus::Idle;
                self.friend_action_at = None;
            }
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
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
                            self.friends_snapshot_dirty = true;
                        }
                        Err(msg) => {
                            log::warn!("social: friends refresh failed: {}", msg);
                            self.friends = FriendsListState::Failed(msg);
                        }
                    }
                }
                SocialEvent::FriendActionDone(status) => {
                    match &status {
                        FriendActionStatus::Failed(reason) => {
                            log::warn!("social: friend-action failed: {}", reason);
                        }
                        other => log::info!("social: friend-action {:?}", other),
                    }
                    self.friend_action = status;
                    self.friend_action_at = Some(now);
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

    /// The account (undashed MC UUID) the current redemption is for.
    pub fn link_redeem_uuid(&self) -> Option<&str> {
        self.link_redeem_uuid.as_deref()
    }

    /// Fire a POST to `/api/launcher/link` with the user's typed code, on
    /// behalf of the account `for_uuid` (`None` when signed out, which
    /// fails locally: a token with no account to store it on would burn
    /// the one-shot code). No-op if a redemption is already in flight.
    /// Result lands in `link_redeem()` via the next `poll()` after the
    /// worker finishes.
    pub fn submit_link_code(&mut self, code: String, for_uuid: Option<&str>) {
        if matches!(self.link_redeem, LinkRedeemStatus::Submitting) {
            return;
        }
        let Some(base) = self.base_for(None, SocialCall::LinkRedeem) else {
            self.link_redeem = LinkRedeemStatus::Failed("social features aren't configured".into());
            return;
        };
        let Some(uuid) = for_uuid else {
            self.link_redeem = LinkRedeemStatus::Failed("sign in with Microsoft first".into());
            return;
        };
        self.link_redeem = LinkRedeemStatus::Submitting;
        self.link_redeem_uuid = Some(uuid.to_string());

        let tx = self.tx.clone();
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
    /// Keeps `link_redeem_uuid`: a redemption cancelled mid-flight still
    /// consumed the code server-side, and its late `Success` is harvested
    /// onto that account.
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
        if mc_uuid.is_empty() {
            return;
        }
        let Some(base) = self.base_for(Some(social_token), SocialCall::Heartbeat) else {
            return;
        };
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

    /// Seconds the current settled friend-action has been showing at
    /// `now`, or `None` while idle / still submitting (a submitting
    /// status has no landing time; callers draw it fully visible).
    pub fn friend_action_age(&self, now: f32) -> Option<f32> {
        self.friend_action_at.map(|at| (now - at).max(0.0))
    }

    // ── Phase H6: live network status ──────────────────────────────────

    /// Latest network-status snapshot, or `None` before the first poll.
    pub fn server_status(&self) -> Option<&ServerStatus> {
        self.server_status.as_ref()
    }

    /// Poll `/api/server-status` if at least
    /// `SERVER_STATUS_REFRESH_INTERVAL_S` has elapsed since the last poll
    /// start. The endpoint is public, but the launcher only calls it once
    /// the active account is linked (`social_token`), per the
    /// offline-first rule. The caller gates this to the main-menu screen
    /// so we don't hammer the endpoint from screens that never show the
    /// widget.
    pub fn maybe_refresh_server_status(&mut self, time: f32, social_token: Option<&str>) {
        let Some(base) = self.base_for(social_token, SocialCall::ServerStatus) else {
            return;
        };
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
        let Some(base) = self.base_for(Some(social_token), SocialCall::Friends) else {
            return;
        };
        if self.friends_in_flight {
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
        let token = social_token.to_string();
        let _ = thread::Builder::new()
            .name("ewo-social-friends-refresh".into())
            .spawn(move || {
                let result = fetch_friends(&base, &token);
                let _ = tx.send(SocialEvent::FriendsRefreshed(result));
            });
    }

    pub fn submit_friend_request_by_name(&mut self, social_token: &str, mc_name: String) {
        self.dispatch_friend_action(social_token, FriendActionPayload::RequestByName(mc_name));
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
        self.dispatch_friend_action(social_token, FriendActionPayload::Remove(discord_id));
    }

    fn dispatch_friend_action(&mut self, social_token: &str, payload: FriendActionPayload) {
        let Some(base) = self.base_for(Some(social_token), SocialCall::FriendAction) else {
            return;
        };
        // Block overlapping mutations — sender of the inflight POST
        // gets the response next tick anyway.
        if matches!(self.friend_action, FriendActionStatus::Submitting) {
            return;
        }
        self.friend_action = FriendActionStatus::Submitting;
        self.friend_action_at = None;
        // Pre-mark the friends list as in_flight so the per-frame
        // 30s tick doesn't kick its own GET while ours is in flight.
        self.friends_in_flight = true;
        let tx = self.tx.clone();
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
    Respond {
        from_discord_id: String,
        accept: bool,
    },
    Remove(String),
}

impl Default for SocialState {
    fn default() -> Self {
        Self::new()
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
    match agent
        .post(&url)
        .set("Authorization", &bearer)
        .send_json(body)
    {
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

    let json: serde_json::Value = resp.into_json().map_err(|e| format!("json parse: {}", e))?;

    fn parse_section(arr: &serde_json::Value) -> Vec<FriendEntry> {
        arr.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let discord_id = item.get("discord_id")?.as_str()?.to_string();
                        let minecraft_uuid = item
                            .get("minecraft_uuid")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let presence = item.get("presence").and_then(|p| {
                            Some(FriendPresence {
                                location: p.get("location")?.as_str()?.to_string(),
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
        online: json
            .get("online")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
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
                        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(str::to_string))
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
                        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(str::to_string))
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
                Ok(_) => FriendActionStatus::Done(if accept {
                    "request accepted".into()
                } else {
                    "request declined".into()
                }),
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
            let url = remove_friend_url(base, &discord_id);
            match agent.delete(&url).set("Authorization", &bearer).call() {
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
                    _ => LinkRedeemStatus::Failed("bot returned a malformed response".into()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> Option<SocialConfig> {
        // `.invalid` never resolves (RFC 2606), so even a gate bug could
        // not reach a real host from a test.
        config_from(Some("https://bot.example.invalid/bot/"), None)
    }

    /// Every request method, called with whatever token it would get.
    /// Each one sets an in-flight flag (or `Submitting`) before it spawns
    /// its worker, so "flag still clear" means "no request issued".
    fn drive_every_call(s: &mut SocialState, token: &str) {
        s.maybe_send_heartbeat(
            100.0,
            "0123456789abcdef0123456789abcdef",
            token,
            HeartbeatLocation::InLauncher {
                screen: "main_menu",
            },
        );
        s.maybe_refresh_friends(100.0, token);
        s.refresh_friends_now(token);
        s.maybe_refresh_server_status(100.0, Some(token).filter(|t| !t.is_empty()));
        s.submit_friend_request_by_name(token, "someone".into());
        s.respond_friend_request(token, "42".into(), true);
        s.remove_friend(token, "42".into());
    }

    fn nothing_in_flight(s: &SocialState) -> bool {
        !s.heartbeat.in_flight
            && !s.friends_in_flight
            && !s.server_status_in_flight
            && matches!(s.friend_action, FriendActionStatus::Idle)
            && matches!(s.friends, FriendsListState::Unknown)
    }

    #[test]
    fn unset_or_empty_base_disables_social() {
        assert_eq!(config_from(None, None), None);
        assert_eq!(config_from(Some(""), Some("lobby.example")), None);
        assert_eq!(config_from(Some("   "), None), None);
    }

    #[test]
    fn base_must_be_https() {
        assert_eq!(
            validate_base("https://bot.example.net/bot/").as_deref(),
            Ok("https://bot.example.net/bot")
        );
        assert_eq!(
            validate_base("  HTTPS://bot.example.net:8443  ").as_deref(),
            Ok("https://bot.example.net:8443")
        );
        for bad in [
            "http://bot.example.net/bot",
            "ftp://bot.example.net",
            "bot.example.net/bot",
            "https://",
            "https:///bot",
            "https://:443/bot",
            "https://user:pw@bot.example.net",
            "https://bot.example.net/bot?x=1",
            "https://bot.example.net/b ot",
        ] {
            assert!(validate_base(bad).is_err(), "{bad} should be rejected");
            assert_eq!(
                config_from(Some(bad), None),
                None,
                "{bad} must disable social"
            );
        }
    }

    #[test]
    fn lobby_is_optional_and_never_defaulted() {
        let c = config_from(Some("https://bot.example.net"), None).unwrap();
        assert_eq!(c.lobby_addr, None);
        let c = config_from(
            Some("https://bot.example.net"),
            Some(" play.example.net:25565 "),
        )
        .unwrap();
        assert_eq!(c.lobby_addr.as_deref(), Some("play.example.net:25565"));
        let c = config_from(Some("https://bot.example.net"), Some("bad host")).unwrap();
        assert_eq!(c.lobby_addr, None);
    }

    #[test]
    fn call_gate_requires_config_then_token() {
        use SocialCall::*;
        let all = [LinkRedeem, ServerStatus, Heartbeat, Friends, FriendAction];
        for call in all {
            assert!(
                !call_allowed(false, Some("tok"), call),
                "{call:?} with no config"
            );
            assert!(!call_allowed(false, None, call), "{call:?} with no config");
        }
        // Configured but unlinked: only the user-initiated redemption.
        for call in all {
            assert_eq!(
                call_allowed(true, None, call),
                call == LinkRedeem,
                "{call:?}"
            );
            assert_eq!(
                call_allowed(true, Some(""), call),
                call == LinkRedeem,
                "{call:?}"
            );
        }
        for call in all {
            assert!(
                call_allowed(true, Some("tok"), call),
                "{call:?} once linked"
            );
        }
    }

    #[test]
    fn disabled_social_issues_no_request() {
        let mut s = SocialState::with_config(None);
        assert!(!s.enabled());
        assert_eq!(s.lobby_addr(), None);
        assert_eq!(s.base_for(Some("tok"), SocialCall::Friends), None);
        drive_every_call(&mut s, "tok");
        assert!(nothing_in_flight(&s));
        s.submit_link_code("123456".into(), Some("0123456789abcdef0123456789abcdef"));
        assert!(matches!(s.link_redeem(), LinkRedeemStatus::Failed(_)));
        assert_eq!(s.link_redeem_uuid(), None);
    }

    #[test]
    fn configured_but_unlinked_issues_no_request() {
        let mut s = SocialState::with_config(configured());
        assert!(s.enabled());
        drive_every_call(&mut s, "");
        assert!(nothing_in_flight(&s));
    }

    #[test]
    fn link_redeem_needs_an_account() {
        let mut s = SocialState::with_config(configured());
        s.submit_link_code("123456".into(), None);
        assert!(matches!(s.link_redeem(), LinkRedeemStatus::Failed(_)));
        assert_eq!(s.link_redeem_uuid(), None);
    }

    #[test]
    fn linked_account_gets_the_base() {
        let s = SocialState::with_config(configured());
        assert_eq!(
            s.base_for(Some("tok"), SocialCall::ServerStatus).as_deref(),
            Some("https://bot.example.invalid/bot")
        );
    }

    fn land(s: &mut SocialState, status: FriendActionStatus, now: f32) {
        s.tx.send(SocialEvent::FriendActionDone(status)).unwrap();
        s.poll(now);
    }

    #[test]
    fn settled_friend_action_expires_after_the_toast_window() {
        let mut s = SocialState::with_config(None);
        land(
            &mut s,
            FriendActionStatus::Done("Request sent".into()),
            10.0,
        );
        assert!(matches!(s.friend_action(), FriendActionStatus::Done(_)));
        assert_eq!(s.friend_action_age(12.5), Some(2.5));
        s.poll(10.0 + FRIEND_ACTION_TOAST_S - 0.01);
        assert!(matches!(s.friend_action(), FriendActionStatus::Done(_)));
        s.poll(10.0 + FRIEND_ACTION_TOAST_S);
        assert!(matches!(s.friend_action(), FriendActionStatus::Idle));
        assert_eq!(s.friend_action_age(99.0), None);
    }

    #[test]
    fn failed_friend_action_expires_too() {
        let mut s = SocialState::with_config(None);
        land(&mut s, FriendActionStatus::Failed("nope".into()), 1.0);
        assert!(matches!(s.friend_action(), FriendActionStatus::Failed(_)));
        s.poll(1.0 + FRIEND_ACTION_TOAST_S + 1.0);
        assert!(matches!(s.friend_action(), FriendActionStatus::Idle));
    }

    #[test]
    fn submitting_friend_action_never_expires() {
        let mut s = SocialState::with_config(None);
        land(&mut s, FriendActionStatus::Done("old".into()), 0.0);
        // A new action starts while the old toast is still up.
        s.friend_action = FriendActionStatus::Submitting;
        s.friend_action_at = None;
        s.poll(1000.0);
        assert!(matches!(s.friend_action(), FriendActionStatus::Submitting));
        assert_eq!(s.friend_action_age(1000.0), None);
    }

    #[test]
    fn a_new_result_restarts_the_toast_clock() {
        let mut s = SocialState::with_config(None);
        land(&mut s, FriendActionStatus::Done("a".into()), 0.0);
        land(&mut s, FriendActionStatus::Done("b".into()), 5.0);
        s.poll(FRIEND_ACTION_TOAST_S + 0.5);
        assert!(matches!(s.friend_action(), FriendActionStatus::Done(m) if m == "b"));
    }

    #[test]
    fn discord_id_is_percent_encoded_in_the_path() {
        let base = "https://bot.example.net/bot";
        assert_eq!(
            remove_friend_url(base, "123456789012345678"),
            "https://bot.example.net/bot/api/friends/123456789012345678"
        );
        assert_eq!(
            remove_friend_url(base, "../../admin?x=1#y"),
            "https://bot.example.net/bot/api/friends/..%2F..%2Fadmin%3Fx%3D1%23y"
        );
    }
}
