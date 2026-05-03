//! `AuthService` — owns the auth state machine and the worker thread.
//!
//! The UI thread holds an `AuthService`. Each frame it calls `poll()` to
//! drain pending `AuthEvent`s and update its in-memory `AuthState`. When
//! the user clicks "Sign in", the UI calls `start_interactive()`, which
//! spawns a worker thread to run the loopback listener + token chain.
//!
//! The worker emits `AuthEvent`s back through an `mpsc::Sender`. The UI
//! only interacts with the worker through this channel — no shared
//! mutable state, no locks.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::chain::{self, ChainStage};
use super::loopback::{self, LoopbackResult};
use super::persistence;
use super::pkce::PkcePair;
use super::{AuthError, MinecraftAccount};

/// Snapshot of the current auth state — what the UI renders. Updated by
/// `poll()` as events arrive.
#[derive(Clone, Debug)]
pub enum AuthState {
    SignedOut,
    /// Sign-in or refresh in flight. Carries the current pipeline stage
    /// label so the UI can show "verifying with Xbox Live…".
    Working(&'static str),
    SignedIn(MinecraftAccount),
    Failed(AuthError),
}

/// Events the worker sends to the UI. The UI maps these to `AuthState`
/// transitions.
#[derive(Debug)]
pub enum AuthEvent {
    Started,
    Progress(ChainStage),
    Success(MinecraftAccount),
    Failure(AuthError),
}

pub struct AuthService {
    state: AuthState,
    rx: Receiver<AuthEvent>,
    tx: Sender<AuthEvent>,
    /// `true` while a worker thread is alive — guards against spawning
    /// concurrent sign-ins (the UI button is also disabled during work,
    /// but this is the authoritative gate).
    busy: bool,
}

impl AuthService {
    /// Construct in `SignedOut`. Caller should immediately attempt
    /// `try_silent_refresh` if a persisted account exists.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            state: AuthState::SignedOut,
            rx,
            tx,
            busy: false,
        }
    }

    pub fn state(&self) -> &AuthState {
        &self.state
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Drain pending events and update `state`. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AuthEvent::Started => {
                    self.state = AuthState::Working(ChainStage::OpeningBrowser.label());
                }
                AuthEvent::Progress(stage) => {
                    self.state = AuthState::Working(stage.label());
                }
                AuthEvent::Success(account) => {
                    log::info!(
                        "auth: signed in as {} (uuid={})",
                        account.name,
                        account.uuid,
                    );
                    persistence::save(&account);
                    self.state = AuthState::SignedIn(account);
                    self.busy = false;
                }
                AuthEvent::Failure(err) => {
                    log::warn!("auth: failed: {:?}", err);
                    self.state = AuthState::Failed(err);
                    self.busy = false;
                }
            }
        }
    }

    /// Begin the interactive (browser) sign-in flow on a worker thread.
    /// No-op if a sign-in is already in flight.
    pub fn start_interactive(&mut self) {
        if self.busy {
            log::info!("auth: already working — ignoring sign-in click");
            return;
        }
        self.busy = true;
        self.state = AuthState::Working(ChainStage::OpeningBrowser.label());
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ewo-auth-interactive".into())
            .spawn(move || run_interactive(tx))
            .expect("spawn auth thread");
    }

    /// Begin a silent refresh from a persisted refresh token. Same
    /// machinery as interactive but skips the browser leg. Useful at
    /// startup; UI reads the in-flight state and shows "refreshing…".
    pub fn start_silent_refresh(&mut self, refresh_token: String) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.state = AuthState::Working(ChainStage::RefreshingMicrosoft.label());
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ewo-auth-refresh".into())
            .spawn(move || run_refresh(tx, refresh_token))
            .expect("spawn auth refresh thread");
    }

    /// Sign out — wipes in-memory + persisted state. Cannot be called
    /// while a sign-in is in flight (caller should ignore the click).
    pub fn sign_out(&mut self) {
        if self.busy {
            return;
        }
        persistence::clear();
        self.state = AuthState::SignedOut;
    }

    /// Hand-set the state to a loaded account (skips the chain). Used at
    /// startup when we have a persisted account and want to render
    /// signed-in UI before/while the silent refresh runs.
    pub fn hydrate(&mut self, account: MinecraftAccount) {
        self.state = AuthState::SignedIn(account);
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
