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

pub struct AuthService {
    /// All signed-in accounts + the active pointer. Persisted to auth.toml.
    store: AccountStore,
    /// Status of the in-flight operation.
    op: AuthOp,
    rx: Receiver<AuthEvent>,
    tx: Sender<AuthEvent>,
    /// `true` while a worker thread is alive — guards against spawning
    /// concurrent chains.
    busy: bool,
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
            busy: false,
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

    /// Drain pending worker events. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AuthEvent::Started => {
                    self.op = AuthOp::Working(ChainStage::OpeningBrowser.label());
                }
                AuthEvent::Progress(stage) => {
                    self.op = AuthOp::Working(stage.label());
                }
                AuthEvent::Success(account) => {
                    log::info!(
                        "auth: signed in as {} (uuid={})",
                        account.name,
                        account.uuid,
                    );
                    let uuid = account.uuid.clone();
                    self.store.upsert(account);
                    self.store.active = Some(uuid);
                    persistence::save_store(&self.store);
                    self.op = AuthOp::Idle;
                    self.busy = false;
                }
                AuthEvent::Failure(err) => {
                    log::warn!("auth: failed: {:?}", err);
                    self.op = AuthOp::Failed(err);
                    self.busy = false;
                }
            }
        }
    }

    /// Begin the interactive (browser) sign-in flow — adds a new account,
    /// or re-authenticates one the user already has. No-op if a chain is
    /// already running.
    pub fn start_interactive(&mut self) {
        if self.busy {
            log::info!("auth: already working — ignoring sign-in request");
            return;
        }
        self.busy = true;
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
        log::info!("auth: removed account {}", uuid);
        // A removed-but-running chain can't be cancelled; its eventual
        // Success would re-add the account. Acceptable — the user can
        // remove it again, and `busy` still gates a fresh op.
        self.refresh_active_token();
    }

    /// Kick a silent refresh for the active account if it has a refresh
    /// token but no live Minecraft token yet, and no chain is running.
    fn refresh_active_token(&mut self) {
        if self.busy {
            return;
        }
        let Some(account) = self.store.active_account() else {
            return;
        };
        if !account.minecraft_token.is_empty() || account.ms_refresh_token.is_empty() {
            return;
        }
        let refresh = account.ms_refresh_token.clone();
        self.busy = true;
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
    if let Err(e) = opener::open(&auth_url) {
        log::warn!(
            "auth: could not open browser ({}); URL is: {}",
            e,
            auth_url,
        );
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
    let result = chain::run_chain_from_auth_code(&code, &redirect_uri, &pkce.verifier, move |stage| {
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
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}
