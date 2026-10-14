//! `AuthService` — owns the account store + the auth worker thread.
//!
//! The UI thread holds an `AuthService`. It is the single source of truth
//! for signed-in accounts (the [`AccountStore`], persisted to `auth.toml`)
//! and for the status of any in-flight auth operation ([`AuthOp`]).
//!
//! Each frame the UI calls `poll()` to drain `AuthEvent`s from the worker
//! and fold them into the store. "Add account" runs the interactive
//! loopback + token chain on a worker thread; `set_active` / `remove`
//! mutate the store and persist; a silent refresh brings an account's
//! short-lived Minecraft token live.
//!
//! The worker emits `AuthEvent`s through an `mpsc::Sender`. No shared
//! mutable state, no locks.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use super::chain::{self, ChainStage};
use super::loopback::{self, LoopbackResult};
use super::persistence::{self, AccountStore};
use super::pkce::PkcePair;
use super::{AuthError, MinecraftAccount};

/// Status of the current auth *operation* — distinct from "is an account
/// signed in", which is a property of the `AccountStore`. The UI renders
/// `Working` as a progress line and `Failed` as an error.
#[derive(Clone, Debug)]
pub enum AuthOp {
    /// No operation in flight.
    Idle,
    /// A sign-in or silent refresh is running. Carries the pipeline stage
    /// label so the UI can show "verifying with Xbox Live…".
    Working(&'static str),
    /// The most recent operation failed.
    Failed(AuthError),
}

/// Events the worker sends to the UI thread.
#[derive(Debug)]
pub enum AuthEvent {
    Started,
    Progress(ChainStage),
    Success(MinecraftAccount),
    Failure(AuthError),
}

/// What the running worker is doing. At most one chain runs at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
enum InFlight {
    /// Interactive browser sign-in. `active_at_start` is the active
    /// pointer when it began: the new account becomes active only if the
    /// user hasn't picked another one meanwhile.
    Interactive { active_at_start: Option<String> },
    /// Silent refresh of the account `uuid`.
    Refresh { uuid: String },
}

/// The active pointer after `job` succeeds with the account `signed_in`.
/// A refresh never moves it (the user may have switched accounts while it
/// ran, and a late `Success` must not switch them back). An interactive
/// sign-in makes the new account active unless the user chose a different
/// active account while the browser flow was open. Removing the account
/// that was active at the start is not a choice: with nothing active, or
/// only the automatic fallback pointer left (`start_gone`), the new account
/// becomes active.
fn active_after_success(
    job: &InFlight,
    current: Option<&str>,
    start_gone: bool,
    signed_in: &str,
) -> Option<String> {
    match job {
        InFlight::Refresh { .. } => current.map(str::to_string),
        InFlight::Interactive { active_at_start } => {
            if current.is_none() || start_gone || current == active_at_start.as_deref() {
                Some(signed_in.to_string())
            } else {
                current.map(str::to_string)
            }
        }
    }
}

/// Whether a finished chain should be followed by a silent refresh of the
/// active account. Never after a failed or cancelled interactive sign-in
/// (the refresh would replace the error the user has to see with a
/// progress line), and never for the refresh that just ended on the still
/// active account (a failing refresh must not loop).
fn follow_up_refresh(finished: &InFlight, succeeded: bool, active: Option<&str>) -> bool {
    match finished {
        InFlight::Refresh { uuid } => active != Some(uuid.as_str()),
        InFlight::Interactive { .. } => succeeded,
    }
}

/// Fold a chain's `Success` into the store (no disk I/O). Returns `false`
/// when the result is dropped: a refresh for an account the user removed
/// while it ran (or whose profile came back different) must not re-add it.
fn apply_success(store: &mut AccountStore, job: &InFlight, account: MinecraftAccount) -> bool {
    if let InFlight::Refresh { uuid } = job {
        let known = store.accounts.iter().any(|a| &a.uuid == uuid);
        if !known || &account.uuid != uuid {
            log::info!("auth: dropped refresh result for {} (account gone)", uuid);
            return false;
        }
    }
    log::info!(
        "auth: signed in as {} (uuid={})",
        account.name,
        account.uuid
    );
    let start_gone = match job {
        InFlight::Interactive {
            active_at_start: Some(start),
        } => !store.accounts.iter().any(|a| &a.uuid == start),
        _ => false,
    };
    let new_active = active_after_success(job, store.active.as_deref(), start_gone, &account.uuid);
    store.upsert(account);
    store.active = new_active;
    true
}

pub struct AuthService {
    /// All signed-in accounts + the active pointer. Persisted to auth.toml.
    store: AccountStore,
    /// Status of the in-flight operation.
    op: AuthOp,
    rx: Receiver<AuthEvent>,
    tx: Sender<AuthEvent>,
    /// `Some` while a worker thread is alive — guards against spawning
    /// concurrent chains and records what the running chain is for.
    in_flight: Option<InFlight>,
}

impl AuthService {
    /// Construct from the persisted store. If an active account exists, a
    /// silent refresh is kicked immediately to bring its token live.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let store = persistence::load_store();
        let mut svc = Self {
            store,
            op: AuthOp::Idle,
            rx,
            tx,
            in_flight: None,
        };
        svc.refresh_active_token();
        svc
    }

    /// All signed-in accounts, in stored order.
    pub fn accounts(&self) -> &[MinecraftAccount] {
        &self.store.accounts
    }

    /// UUIDs of all signed-in accounts, in order — the Settings input
    /// handlers use this to hit-test the account rows.
    pub fn account_uuids(&self) -> Vec<String> {
        self.store.accounts.iter().map(|a| a.uuid.clone()).collect()
    }

    /// The active account — the one launches use. `None` when signed out.
    pub fn active(&self) -> Option<&MinecraftAccount> {
        self.store.active_account()
    }

    /// Status of the in-flight auth operation.
    pub fn op(&self) -> &AuthOp {
        &self.op
    }

    /// How a launch right now should authenticate (see
    /// [`super::launch_auth`]).
    pub fn launch_auth(&self) -> super::LaunchAuth {
        super::launch_auth(self.active(), Instant::now())
    }

    /// Whether any auth chain (sign-in or refresh) is running.
    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Start a silent refresh of the active account's Minecraft token
    /// when it is missing or within [`super::TOKEN_REFRESH_MARGIN`] of
    /// expiry, ahead of a launch. Returns `true` if a refresh was started;
    /// `false` if a chain is already running (wait for it), the token
    /// doesn't need one, or there is no refresh token to use.
    pub fn start_launch_refresh(&mut self) -> bool {
        if self.in_flight.is_some() {
            return false;
        }
        let Some(account) = self.store.active_account() else {
            return false;
        };
        if account.ms_refresh_token.is_empty()
            || !super::token_needs_refresh(
                &account.minecraft_token,
                account.minecraft_token_expires_at,
                Instant::now(),
            )
        {
            return false;
        }
        let (uuid, refresh) = (account.uuid.clone(), account.ms_refresh_token.clone());
        self.spawn_refresh(uuid, refresh);
        true
    }

    /// Drain pending worker events. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AuthEvent::Started => {
                    let stage = match self.in_flight {
                        Some(InFlight::Refresh { .. }) => ChainStage::RefreshingMicrosoft,
                        _ => ChainStage::OpeningBrowser,
                    };
                    self.op = AuthOp::Working(stage.label());
                }
                AuthEvent::Progress(stage) => {
                    self.op = AuthOp::Working(stage.label());
                }
                AuthEvent::Success(account) => {
                    self.op = AuthOp::Idle;
                    let Some(job) = self.in_flight.take() else {
                        log::warn!("auth: stray Success with no chain running; ignored");
                        continue;
                    };
                    if apply_success(&mut self.store, &job, account) {
                        persistence::save_store(&self.store);
                    }
                    self.after_chain(&job, true);
                }
                AuthEvent::Failure(err) => {
                    log::warn!("auth: failed: {:?}", err);
                    self.op = AuthOp::Failed(err);
                    if let Some(job) = self.in_flight.take() {
                        self.after_chain(&job, false);
                    }
                }
            }
        }
    }

    /// A chain just finished. If the user switched to another account while
    /// it ran (`set_active` can't start a refresh while busy), bring that
    /// account's token live now (see [`follow_up_refresh`]).
    fn after_chain(&mut self, finished: &InFlight, succeeded: bool) {
        if follow_up_refresh(finished, succeeded, self.store.active.as_deref()) {
            self.refresh_active_token();
        }
    }

    /// Begin the interactive (browser) sign-in flow — adds a new account,
    /// or re-authenticates one the user already has. No-op if a chain is
    /// already running.
    pub fn start_interactive(&mut self) {
        if self.in_flight.is_some() {
            log::info!("auth: already working — ignoring sign-in request");
            return;
        }
        self.in_flight = Some(InFlight::Interactive {
            active_at_start: self.store.active.clone(),
        });
        self.op = AuthOp::Working(ChainStage::OpeningBrowser.label());
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ewo-auth-interactive".into())
            .spawn(move || run_interactive(tx))
            .expect("spawn auth thread");
    }

    /// Make the account with `uuid` active. Persists, and silently
    /// refreshes its token if it isn't already live. No-op if `uuid`
    /// isn't a known account or is already active.
    pub fn set_active(&mut self, uuid: &str) {
        if self.store.active.as_deref() == Some(uuid) {
            return;
        }
        if !self.store.accounts.iter().any(|a| a.uuid == uuid) {
            return;
        }
        self.store.active = Some(uuid.to_string());
        persistence::save_store(&self.store);
        log::info!("auth: active account -> {}", uuid);
        self.refresh_active_token();
    }

    /// Remove the account with `uuid`. The active pointer falls back to
    /// the first remaining account (see `AccountStore::remove`); its token
    /// is refreshed if needed.
    pub fn remove(&mut self, uuid: &str) {
        self.store.remove(uuid);
        persistence::save_store(&self.store);
        persistence::forget_account_secrets(uuid);
        log::info!("auth: removed account {}", uuid);
        // A running chain can't be cancelled. A refresh for the removed
        // account is dropped when it lands (see `poll`); an interactive
        // sign-in that lands on it re-adds it, since the user just signed
        // into it again.
        self.refresh_active_token();
    }

    /// Phase H2: persist a social_token against an existing account.
    /// Called when the launcher-link modal successfully completes — the
    /// bot returned a fresh token, we save it next to the account so
    /// future bot calls authenticate as this user.
    pub fn set_social_token(&mut self, uuid: &str, token: String) {
        if self.store.set_social_token(uuid, token) {
            persistence::save_store(&self.store);
            log::info!("auth: stored social token for {}", uuid);
        } else {
            log::warn!("auth: ignored social_token for unknown uuid {}", uuid);
        }
    }

    /// Phase H2: read the social_token for the account with `uuid`.
    pub fn social_token(&self, uuid: &str) -> Option<&str> {
        self.store
            .accounts
            .iter()
            .find(|a| a.uuid == uuid)
            .and_then(|a| a.social_token.as_deref())
    }

    /// Kick a silent refresh for the active account if it has a refresh
    /// token but no live Minecraft token yet, and no chain is running.
    /// (A live token close to expiry is refreshed by the launch path,
    /// [`Self::start_launch_refresh`].)
    fn refresh_active_token(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let Some(account) = self.store.active_account() else {
            return;
        };
        if !account.minecraft_token.is_empty() || account.ms_refresh_token.is_empty() {
            return;
        }
        let (uuid, refresh) = (account.uuid.clone(), account.ms_refresh_token.clone());
        self.spawn_refresh(uuid, refresh);
    }

    fn spawn_refresh(&mut self, uuid: String, refresh: String) {
        self.in_flight = Some(InFlight::Refresh { uuid });
        self.op = AuthOp::Working(ChainStage::RefreshingMicrosoft.label());
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ewo-auth-refresh".into())
            .spawn(move || run_refresh(tx, refresh))
            .expect("spawn auth refresh thread");
    }
}

fn run_interactive(tx: Sender<AuthEvent>) {
    let _ = tx.send(AuthEvent::Started);

    // Bind the loopback listener first so we know the redirect URI and
    // can fail fast if the OS won't give us a port.
    let (server, redirect_uri) = match loopback::bind() {
        Ok(b) => b,
        Err(e) => {
            let _ = tx.send(AuthEvent::Failure(AuthError::Other(format!(
                "loopback bind: {}",
                e
            ))));
            return;
        }
    };

    let pkce = PkcePair::new();
    let state_param = random_state();
    let auth_url = chain::build_auth_url(&redirect_uri, &pkce.challenge, &state_param);

    log::info!("auth: redirect_uri = {}", redirect_uri);
    log::debug!("auth: full auth URL = {}", auth_url);

    let _ = tx.send(AuthEvent::Progress(ChainStage::OpeningBrowser));
    if let Err(e) = crate::util::open_url(&auth_url) {
        log::warn!("auth: could not open browser ({}); URL is: {}", e, auth_url,);
        // Don't fail outright — print the URL so the user can paste it.
        // A separate error variant ("browser open failed, paste URL")
        // would be nicer UX; punted for now.
    }

    let _ = tx.send(AuthEvent::Progress(ChainStage::WaitingForRedirect));
    let code = match loopback::wait_for_redirect(&server, &state_param) {
        LoopbackResult::Code(c) => c,
        LoopbackResult::Cancelled => {
            let _ = tx.send(AuthEvent::Failure(AuthError::UserCancelled));
            return;
        }
        LoopbackResult::TimedOut => {
            let _ = tx.send(AuthEvent::Failure(AuthError::UserCancelled));
            return;
        }
    };

    let progress_tx = tx.clone();
    let result =
        chain::run_chain_from_auth_code(&code, &redirect_uri, &pkce.verifier, move |stage| {
            let _ = progress_tx.send(AuthEvent::Progress(stage));
        });

    match result {
        Ok(account) => {
            let _ = tx.send(AuthEvent::Success(account));
        }
        Err(err) => {
            let _ = tx.send(AuthEvent::Failure(err));
        }
    }
}

fn run_refresh(tx: Sender<AuthEvent>, refresh_token: String) {
    let _ = tx.send(AuthEvent::Started);
    let progress_tx = tx.clone();
    let result = chain::run_chain_from_refresh(&refresh_token, move |stage| {
        let _ = progress_tx.send(AuthEvent::Progress(stage));
    });
    match result {
        Ok(account) => {
            let _ = tx.send(AuthEvent::Success(account));
        }
        Err(err) => {
            let _ = tx.send(AuthEvent::Failure(err));
        }
    }
}

/// Random URL-safe token used as the OAuth `state` parameter (CSRF guard).
/// 32 chars is more than enough — the OAuth spec only requires
/// "non-guessable", typical implementations use 8-32 alphanumerics.
fn random_state() -> String {
    use rand::RngExt;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..32)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(uuid: &str, token: &str) -> MinecraftAccount {
        MinecraftAccount {
            name: format!("name-{uuid}"),
            uuid: uuid.into(),
            minecraft_token: token.into(),
            minecraft_token_expires_at: None,
            ms_refresh_token: format!("refresh-{uuid}"),
            social_token: None,
        }
    }

    fn store(active: &str) -> AccountStore {
        AccountStore {
            active: Some(active.into()),
            accounts: vec![account("a", ""), account("b", "")],
            ..Default::default()
        }
    }

    fn refresh(uuid: &str) -> InFlight {
        InFlight::Refresh { uuid: uuid.into() }
    }

    #[test]
    fn late_refresh_for_a_does_not_override_a_switch_to_b() {
        // Refresh for A started while A was active; the user then chose B.
        let mut s = store("b");
        assert!(apply_success(&mut s, &refresh("a"), account("a", "fresh")));
        assert_eq!(s.active.as_deref(), Some("b"));
        let a = s.accounts.iter().find(|x| x.uuid == "a").unwrap();
        assert_eq!(a.minecraft_token, "fresh", "A's token still lands");
    }

    #[test]
    fn refresh_for_a_removed_account_is_dropped() {
        let mut s = AccountStore {
            active: Some("b".into()),
            accounts: vec![account("b", "")],
            ..Default::default()
        };
        assert!(!apply_success(&mut s, &refresh("a"), account("a", "fresh")));
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.active.as_deref(), Some("b"));
    }

    #[test]
    fn interactive_sign_in_becomes_active_unless_user_switched() {
        let job = InFlight::Interactive {
            active_at_start: Some("a".into()),
        };
        let mut s = store("a");
        assert!(apply_success(&mut s, &job, account("c", "t")));
        assert_eq!(s.active.as_deref(), Some("c"));

        // The user picked B while the browser flow was open.
        let mut s = store("b");
        assert!(apply_success(&mut s, &job, account("c", "t")));
        assert_eq!(s.active.as_deref(), Some("b"));
        assert!(s.accounts.iter().any(|x| x.uuid == "c"), "C is still added");

        // First sign-in on an empty store.
        let job = InFlight::Interactive {
            active_at_start: None,
        };
        let mut s = AccountStore::default();
        assert!(apply_success(&mut s, &job, account("a", "t")));
        assert_eq!(s.active.as_deref(), Some("a"));
    }

    #[test]
    fn interactive_sign_in_after_the_active_account_was_removed_becomes_active() {
        let job = InFlight::Interactive {
            active_at_start: Some("a".into()),
        };
        // The only account was removed while the browser flow was open.
        let mut s = AccountStore::default();
        assert!(apply_success(&mut s, &job, account("c", "t")));
        assert_eq!(s.active.as_deref(), Some("c"), "nothing active: take it");

        // A (active at the start) was removed and the pointer fell back to B,
        // which the user never chose.
        let mut s = AccountStore {
            active: Some("b".into()),
            accounts: vec![account("b", "")],
            ..Default::default()
        };
        assert!(apply_success(&mut s, &job, account("c", "t")));
        assert_eq!(s.active.as_deref(), Some("c"));

        // The pure rule, including the unchanged cases.
        assert_eq!(
            active_after_success(&job, None, true, "c").as_deref(),
            Some("c")
        );
        assert_eq!(
            active_after_success(&job, Some("b"), false, "c").as_deref(),
            Some("b"),
            "a deliberate switch to B stays"
        );
    }

    #[test]
    fn a_failed_interactive_sign_in_leaves_its_error_alone() {
        let interactive = InFlight::Interactive {
            active_at_start: Some("a".into()),
        };
        // A failed or cancelled browser sign-in keeps its error on screen.
        assert!(!follow_up_refresh(&interactive, false, Some("a")));
        assert!(!follow_up_refresh(&interactive, false, None));
        assert!(follow_up_refresh(&interactive, true, Some("a")));
        // A refresh never re-runs for the account it just handled...
        assert!(!follow_up_refresh(&refresh("a"), false, Some("a")));
        assert!(!follow_up_refresh(&refresh("a"), true, Some("a")));
        // ...but a switch to another account while it ran gets one.
        assert!(follow_up_refresh(&refresh("a"), true, Some("b")));
    }
}
