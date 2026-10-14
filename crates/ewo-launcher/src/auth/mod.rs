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
//! `<config>/EwoClient/auth.toml` so the user signs in once (sealed with
//! DPAPI on Windows, see [`secret`]). On startup we attempt a silent
//! refresh in the background; if it fails (network down, refresh token
//! expired/revoked), we fall back to "Sign in" UX. Before every launch
//! the Minecraft token is refreshed again if it is missing or close to
//! expiry ([`launch_auth`]).

pub mod chain;
pub mod loopback;
pub mod persistence;
pub mod pkce;
pub mod secret;
pub mod service;

pub use service::{AuthOp, AuthService};

use std::time::{Duration, Instant};

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

/// Refresh the Minecraft token before a launch when it has less than this
/// left. A game session outlives its launch, so a token that expires a
/// minute after the JVM starts is as bad as an expired one.
pub const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);

/// Assumed Minecraft token lifetime when the login response omits
/// `expires_in` (Minecraft Services documents and sends 86400 s).
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// A signed-in Minecraft account, persisted to disk and held in memory
/// while the user is signed in. The Minecraft access token is short-lived
/// (~24h) — kept in memory only; we re-derive it from the refresh token
/// on startup, and before a launch when it is missing or close to expiry.
#[derive(Clone, Serialize, Deserialize)]
pub struct MinecraftAccount {
    /// Minecraft display name (e.g. "Notch").
    pub name: String,
    /// Minecraft UUID (32 hex chars, no dashes).
    pub uuid: String,
    /// Short-lived bearer token to pass as `--accessToken` to the JVM.
    /// Skipped from persistence — re-derived from `ms_refresh_token`.
    #[serde(skip)]
    pub minecraft_token: String,
    /// When `minecraft_token` expires (from the login's `expires_in`).
    /// In memory only, like the token itself. `None` = no live token.
    #[serde(skip)]
    pub minecraft_token_expires_at: Option<Instant>,
    /// Microsoft refresh token. **Sensitive — treat as a credential.**
    /// Sealed at rest by [`secret`] (DPAPI on Windows; a `0600`
    /// plaintext file elsewhere).
    pub ms_refresh_token: String,
    /// Phase H2: per-user bearer token for the Frogsy bot API.
    /// `None` until the user completes the in-game `/launcher-link`
    /// flow + redeems the code in the launcher's link modal. Sensitive
    /// — single revocable credential per launcher install.
    /// `default` so old auth.toml files (no field) deserialize cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub social_token: Option<String>,
}

/// Hand-written so no `{:?}` can ever leak a credential into a log.
impl std::fmt::Debug for MinecraftAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinecraftAccount")
            .field("name", &self.name)
            .field("uuid", &self.uuid)
            .field("minecraft_token", &"<redacted>")
            .field("ms_refresh_token", &"<redacted>")
            .field("social_token", &self.social_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// When a token issued at `now` with the login response's `expires_in`
/// (seconds) expires. A missing value assumes [`DEFAULT_TOKEN_LIFETIME`].
pub fn token_expiry(now: Instant, expires_in: Option<u64>) -> Instant {
    let life = expires_in.map(Duration::from_secs).unwrap_or(DEFAULT_TOKEN_LIFETIME);
    now.checked_add(life).unwrap_or(now + DEFAULT_TOKEN_LIFETIME)
}

/// Whether a Minecraft token should be refreshed before use: it is
/// missing, its expiry is unknown, or it has [`TOKEN_REFRESH_MARGIN`] or
/// less left.
pub fn token_needs_refresh(token: &str, expires_at: Option<Instant>, now: Instant) -> bool {
    match expires_at {
        Some(exp) if !token.is_empty() => {
            exp.saturating_duration_since(now) <= TOKEN_REFRESH_MARGIN
        }
        _ => true,
    }
}

/// How a launch should authenticate, decided from the active account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchAuth {
    /// No account is signed in: launch with an offline profile, as always.
    Offline,
    /// The active account's token is good: launch with it.
    Ready,
    /// The token is missing or close to expiry and a refresh token
    /// exists: refresh first, then launch.
    NeedsRefresh,
    /// Signed in, but no usable token can be had. Fail the launch with
    /// this message; never fall back to offline for a signed-in account.
    Unavailable(&'static str),
}

/// Decide how a launch authenticates. A signed-in account never launches
/// offline: it launches with a live token, refreshes first, or fails.
pub fn launch_auth(active: Option<&MinecraftAccount>, now: Instant) -> LaunchAuth {
    let Some(a) = active else {
        return LaunchAuth::Offline;
    };
    if !token_needs_refresh(&a.minecraft_token, a.minecraft_token_expires_at, now) {
        return LaunchAuth::Ready;
    }
    if !a.ms_refresh_token.is_empty() {
        return LaunchAuth::NeedsRefresh;
    }
    // Nothing to refresh with. A token inside the margin but not yet
    // expired still beats failing.
    let still_valid = !a.minecraft_token.is_empty()
        && a.minecraft_token_expires_at.is_some_and(|exp| now < exp);
    if still_valid {
        LaunchAuth::Ready
    } else {
        LaunchAuth::Unavailable(
            "your Microsoft session has expired; sign in again in Settings → Account",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(token: &str, expires_at: Option<Instant>, refresh: &str) -> MinecraftAccount {
        MinecraftAccount {
            name: "Steve".into(),
            uuid: "0123456789abcdef0123456789abcdef".into(),
            minecraft_token: token.into(),
            minecraft_token_expires_at: expires_at,
            ms_refresh_token: refresh.into(),
            social_token: None,
        }
    }

    #[test]
    fn expiry_uses_expires_in_or_a_day() {
        let now = Instant::now();
        assert_eq!(token_expiry(now, Some(3600)), now + Duration::from_secs(3600));
        assert_eq!(token_expiry(now, None), now + DEFAULT_TOKEN_LIFETIME);
    }

    #[test]
    fn needs_refresh_inside_the_margin() {
        let now = Instant::now();
        let hour = Duration::from_secs(3600);
        assert!(token_needs_refresh("", Some(now + hour), now), "missing token");
        assert!(token_needs_refresh("t", None, now), "unknown expiry");
        assert!(!token_needs_refresh("t", Some(now + hour), now), "an hour left");
        assert!(
            !token_needs_refresh("t", Some(now + TOKEN_REFRESH_MARGIN + Duration::from_secs(1)), now),
            "just outside the margin"
        );
        assert!(token_needs_refresh("t", Some(now + TOKEN_REFRESH_MARGIN), now), "at the margin");
        assert!(token_needs_refresh("t", Some(now + Duration::from_secs(60)), now), "a minute left");
        assert!(token_needs_refresh("t", Some(now), now), "expired now");
        // A day-old token (the launcher left open) is expired.
        let issued = now;
        let later = issued + DEFAULT_TOKEN_LIFETIME + Duration::from_secs(1);
        assert!(token_needs_refresh("t", Some(token_expiry(issued, Some(86_400))), later));
    }

    #[test]
    fn signed_out_launches_offline() {
        assert_eq!(launch_auth(None, Instant::now()), LaunchAuth::Offline);
    }

    #[test]
    fn live_token_launches() {
        let now = Instant::now();
        let a = account("t", Some(now + Duration::from_secs(3600)), "r");
        assert_eq!(launch_auth(Some(&a), now), LaunchAuth::Ready);
    }

    #[test]
    fn signed_in_without_token_refreshes_never_offline() {
        let now = Instant::now();
        // Refresh in flight or failed at startup: no live token yet.
        let a = account("", None, "r");
        assert_eq!(launch_auth(Some(&a), now), LaunchAuth::NeedsRefresh);
        // Near expiry.
        let a = account("t", Some(now + Duration::from_secs(60)), "r");
        assert_eq!(launch_auth(Some(&a), now), LaunchAuth::NeedsRefresh);
        // Expired with nothing to refresh with: an error, not offline.
        let a = account("t", Some(now), "");
        assert!(matches!(launch_auth(Some(&a), now), LaunchAuth::Unavailable(_)));
        let a = account("", None, "");
        assert!(matches!(launch_auth(Some(&a), now), LaunchAuth::Unavailable(_)));
    }

    #[test]
    fn unrefreshable_token_inside_margin_still_launches() {
        let now = Instant::now();
        let a = account("t", Some(now + Duration::from_secs(60)), "");
        assert_eq!(launch_auth(Some(&a), now), LaunchAuth::Ready);
    }
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
