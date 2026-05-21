//! Microsoft auth pipeline for Minecraft (v2 phase A).
//!
//! See `CLAUDE.md` § "v2 plan / Phase A" for the full design. The pipeline
//! is **four token exchanges plus a profile fetch**:
//!
//!   1. Microsoft OAuth (authorization code + PKCE)
//!   2. Xbox Live (`user.auth.xboxlive.com`)
//!   3. XSTS (`xsts.auth.xboxlive.com`)
//!   4. Minecraft Services (`api.minecraftservices.com/authentication/login_with_xbox`)
//!   5. Profile fetch (`api.minecraftservices.com/minecraft/profile`)
//!
//! The chain runs on a background thread; the UI thread polls a `mpsc`
//! channel for `AuthEvent`s and renders the current `AuthState`. No
//! tokio/smol — sync HTTP via `ureq` per the CLAUDE.md non-negotiables.
//!
//! Persistence: refresh tokens are saved at
//! `<config>/EwoClient/auth.toml` so the user signs in once. On startup
//! we attempt a silent refresh in the background; if it fails (network
//! down, refresh token expired/revoked), we fall back to "Sign in" UX.

pub mod chain;
pub mod loopback;
pub mod persistence;
pub mod pkce;
pub mod service;

pub use service::{AuthService, AuthState};

use serde::{Deserialize, Serialize};

/// Public/native client ID for the EwoClient Entra app registration.
/// The user-supplied UUID is harmless to embed in source — public clients
/// have no secret, and the redirect URI binding is enforced server-side.
pub const CLIENT_ID: &str = "f901fc74-7e36-439d-80a8-c2e548f47fdc";

/// `consumers` tenant — personal Microsoft accounts. Minecraft accounts
/// live here; using `common` would also accept work/school accounts which
/// can't own Minecraft licenses.
pub const TENANT: &str = "consumers";

/// Scopes the launcher needs. `XboxLive.signin` is the bridge into the
/// Xbox auth chain; `offline_access` is what makes the response include a
/// refresh token.
pub const SCOPES: &str = "XboxLive.signin offline_access";

/// A signed-in Minecraft account, persisted to disk and held in memory
/// while the user is signed in. The Minecraft access token is short-lived
/// (~24h) — kept in memory only; we re-derive it from the refresh token
/// on startup or when expired.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MinecraftAccount {
    /// Minecraft display name (e.g. "Notch").
    pub name: String,
    /// Minecraft UUID (32 hex chars, no dashes).
    pub uuid: String,
    /// Short-lived bearer token to pass as `--accessToken` to the JVM.
    /// Skipped from persistence — re-derived from `ms_refresh_token`.
    #[serde(skip)]
    pub minecraft_token: String,
    /// Microsoft refresh token. **Sensitive — treat as a credential.**
    /// Currently stored plaintext in `auth.toml`; encrypting at rest is
    /// a follow-up if/when we ship this binary widely.
    pub ms_refresh_token: String,
}

/// Errors surfaced to the UI. The `String` payloads are user-facing
/// where they need to be ("You don't appear to own Minecraft on this
/// account.") and developer-facing otherwise (HTTP status + body).
#[derive(Clone, Debug)]
pub enum AuthError {
    /// User cancelled the browser flow (closed window without signing in,
    /// or the loopback listener timed out waiting for a redirect).
    UserCancelled,
    /// Network call failed at some stage of the chain.
    Network(String),
    /// XSTS returned a known XErr code we can render specifically.
    /// See https://wiki.vg/Microsoft_Authentication_Scheme#XSTS_response_errors
    XstsBlocked(XstsBlock),
    /// The Microsoft account doesn't own Minecraft on Java edition.
    NoMinecraftLicense,
    /// Anything else — bubbled up from the chain with the offending
    /// HTTP status + body so we can debug from logs.
    Other(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum XstsBlock {
    /// `2148916233` — no Xbox account associated with this MS account.
    NoXboxAccount,
    /// `2148916235` — region blocks Xbox Live.
    RegionBlocked,
    /// `2148916236` / `2148916237` — adult verification needed.
    AdultVerificationNeeded,
    /// `2148916238` — child account, needs family setup.
    ChildAccount,
    Other(i64),
}

impl XstsBlock {
    pub fn from_xerr(code: i64) -> Self {
        match code {
            2148916233 => XstsBlock::NoXboxAccount,
            2148916235 => XstsBlock::RegionBlocked,
            2148916236 | 2148916237 => XstsBlock::AdultVerificationNeeded,
            2148916238 => XstsBlock::ChildAccount,
            other => XstsBlock::Other(other),
        }
    }

    pub fn user_message(self) -> &'static str {
        match self {
            XstsBlock::NoXboxAccount => {
                "This Microsoft account has no Xbox profile yet. Visit \
                 xbox.com to create one, then try signing in again."
            }
            XstsBlock::RegionBlocked => {
                "Xbox Live isn't available in your region. Minecraft launching \
                 isn't supported here."
            }
            XstsBlock::AdultVerificationNeeded => {
                "Your account needs adult verification. Visit \
                 account.microsoft.com to complete it."
            }
            XstsBlock::ChildAccount => {
                "This child account needs to be added to a family group by an \
                 adult before it can sign in to Minecraft."
            }
            XstsBlock::Other(_) => {
                "Xbox Live rejected this account. Check that your Microsoft \
                 account is in good standing."
            }
        }
    }
}
