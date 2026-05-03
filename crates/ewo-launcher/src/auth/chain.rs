//! The four-step Microsoft → Xbox Live → XSTS → Minecraft Services
//! token chain, plus the profile fetch that gives us the UUID + display
//! name. Reference: https://wiki.vg/Microsoft_Authentication_Scheme
//!
//! Each function returns the next step's input — you call them in order.
//! `run_chain_from_auth_code` and `run_chain_from_refresh` are the two
//! public entry points; the chain wraps the rest.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{AuthError, MinecraftAccount, XstsBlock, CLIENT_ID, SCOPES, TENANT};

// Endpoint URLs — pulled out so the chain reads top-down.
const MS_TOKEN_URL: &str = "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token";
const MS_AUTHORIZE_URL: &str = "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_OWNERSHIP_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// HTTP request timeout. Generous because the Xbox Live endpoints are
/// occasionally slow + we never want a fast-flapping retry loop.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

fn ureq_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("EwoClient/0.1 (+https://github.com/valtterisaarinen/ewoclient)")
        .build()
}

/// Build the auth URL the user opens in their browser. State is a random
/// string we round-trip for CSRF protection.
pub fn build_auth_url(redirect_uri: &str, code_challenge: &str, state: &str) -> String {
    let url = MS_AUTHORIZE_URL.replace("{tenant}", TENANT);
    format!(
        "{base}?client_id={cid}&response_type=code&redirect_uri={redir}\
         &response_mode=query&scope={scope}&state={state}\
         &code_challenge={chal}&code_challenge_method=S256\
         &prompt=select_account",
        base = url,
        cid = CLIENT_ID,
        redir = urlencoding::encode(redirect_uri),
        scope = urlencoding::encode(SCOPES),
        state = urlencoding::encode(state),
        chal = urlencoding::encode(code_challenge),
    )
}

#[derive(Deserialize, Debug)]
struct MsTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

/// Step 1a: exchange the auth code we caught at the loopback for an MS
/// access + refresh token pair.
fn exchange_auth_code(
    agent: &ureq::Agent,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<MsTokenResponse, AuthError> {
    let url = MS_TOKEN_URL.replace("{tenant}", TENANT);
    let body = format!(
        "client_id={cid}&grant_type=authorization_code&code={code}\
         &redirect_uri={redir}&scope={scope}&code_verifier={ver}",
        cid = urlencoding::encode(CLIENT_ID),
        code = urlencoding::encode(code),
        redir = urlencoding::encode(redirect_uri),
        scope = urlencoding::encode(SCOPES),
        ver = urlencoding::encode(code_verifier),
    );
    post_form_json(agent, &url, &body)
}

/// Step 1b: alternate entry point — silent refresh using the previously
/// persisted refresh token. No browser involved.
fn refresh_ms_token(agent: &ureq::Agent, refresh_token: &str) -> Result<MsTokenResponse, AuthError> {
    let url = MS_TOKEN_URL.replace("{tenant}", TENANT);
    let body = format!(
        "client_id={cid}&grant_type=refresh_token&refresh_token={rt}&scope={scope}",
        cid = urlencoding::encode(CLIENT_ID),
        rt = urlencoding::encode(refresh_token),
        scope = urlencoding::encode(SCOPES),
    );
    post_form_json(agent, &url, &body)
}

#[derive(Deserialize, Debug)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: XblDisplayClaims,
}

#[derive(Deserialize, Debug)]
struct XblDisplayClaims {
    xui: Vec<XuiEntry>,
}

#[derive(Deserialize, Debug)]
struct XuiEntry {
    uhs: String,
}

/// Step 2: exchange the MS access token for an XBL token + user hash.
fn xbl_authenticate(agent: &ureq::Agent, ms_access_token: &str) -> Result<XblResponse, AuthError> {
    let payload = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={}", ms_access_token),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    post_json(agent, XBL_AUTH_URL, &payload)
}

#[derive(Deserialize, Debug)]
struct XstsResponse {
    #[serde(rename = "Token")]
    token: String,
}

#[derive(Deserialize, Debug)]
struct XstsErrorResponse {
    #[serde(rename = "XErr")]
    xerr: i64,
    #[serde(rename = "Message", default)]
    message: String,
}

/// Step 3: exchange the XBL token for an XSTS token. This is where most
/// account-state errors surface (region blocked, no Xbox profile, etc.) —
/// XSTS returns 401 with a JSON body containing `XErr`.
fn xsts_authorize(agent: &ureq::Agent, xbl_token: &str) -> Result<XstsResponse, AuthError> {
    let payload = json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });

    // We need to peek at 401 bodies for the XErr code — bypass ureq's
    // default error-on-non-2xx by handling the error variant ourselves.
    let res = agent
        .post(XSTS_AUTH_URL)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_json(&payload);
    match res {
        Ok(r) => {
            let body: XstsResponse = r
                .into_json()
                .map_err(|e| AuthError::Other(format!("XSTS body parse: {}", e)))?;
            Ok(body)
        }
        Err(ureq::Error::Status(401, resp)) => {
            // Try to parse the XErr code from the error body.
            let parsed = resp.into_json::<XstsErrorResponse>();
            match parsed {
                Ok(err) => {
                    log::warn!("auth: XSTS 401 XErr={} message={}", err.xerr, err.message);
                    Err(AuthError::XstsBlocked(XstsBlock::from_xerr(err.xerr)))
                }
                Err(e) => Err(AuthError::Other(format!("XSTS 401 + unparsable body: {}", e))),
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(AuthError::Other(format!("XSTS {}: {}", code, body)))
        }
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

#[derive(Deserialize, Debug)]
struct McLoginResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

/// Step 4: exchange XSTS + UserHash for a Minecraft access token.
fn mc_login(agent: &ureq::Agent, xsts_token: &str, uhs: &str) -> Result<McLoginResponse, AuthError> {
    let payload = json!({
        "identityToken": format!("XBL3.0 x={};{}", uhs, xsts_token),
    });
    post_json(agent, MC_LOGIN_URL, &payload)
}

#[derive(Deserialize, Debug)]
struct McProfile {
    id: String,
    name: String,
}

/// Step 5: GET the Minecraft profile (UUID, display name). Also acts as
/// our license check — profile call returns 404 if the account doesn't
/// own the game.
fn mc_profile(agent: &ureq::Agent, mc_token: &str) -> Result<McProfile, AuthError> {
    let res = agent
        .get(MC_PROFILE_URL)
        .set("Authorization", &format!("Bearer {}", mc_token))
        .set("Accept", "application/json")
        .call();
    match res {
        Ok(r) => r
            .into_json()
            .map_err(|e| AuthError::Other(format!("profile parse: {}", e))),
        Err(ureq::Error::Status(404, _)) => Err(AuthError::NoMinecraftLicense),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(AuthError::Other(format!("profile {}: {}", code, body)))
        }
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

/// One-shot ownership check. On modern accounts the profile-404 check is
/// sufficient; this is here as a documented option for diagnostic logging
/// — not called in the happy path. Kept private to avoid encouraging
/// extra network calls in the hot path.
#[allow(dead_code)]
fn mc_owns_game(agent: &ureq::Agent, mc_token: &str) -> Result<bool, AuthError> {
    #[derive(Deserialize)]
    struct Resp {
        items: Vec<serde_json::Value>,
    }
    let r: Resp = agent
        .get(MC_OWNERSHIP_URL)
        .set("Authorization", &format!("Bearer {}", mc_token))
        .call()
        .map_err(|e| AuthError::Network(e.to_string()))?
        .into_json()
        .map_err(|e| AuthError::Other(format!("ownership parse: {}", e)))?;
    Ok(!r.items.is_empty())
}

/// Run the entire chain starting from a fresh auth code (loopback flow).
/// Reports progress via the supplied callback so the UI can render
/// stage labels ("verifying with Xbox Live...", "fetching Minecraft profile...").
pub fn run_chain_from_auth_code(
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    progress: impl Fn(ChainStage),
) -> Result<MinecraftAccount, AuthError> {
    let agent = ureq_agent();

    progress(ChainStage::ExchangingAuthCode);
    let ms = exchange_auth_code(&agent, code, redirect_uri, code_verifier)?;

    finish_chain(&agent, &ms, progress)
}

/// Run the chain starting from a persisted refresh token (silent path on
/// app startup).
pub fn run_chain_from_refresh(
    refresh_token: &str,
    progress: impl Fn(ChainStage),
) -> Result<MinecraftAccount, AuthError> {
    let agent = ureq_agent();

    progress(ChainStage::RefreshingMicrosoft);
    let ms = refresh_ms_token(&agent, refresh_token)?;

    finish_chain(&agent, &ms, progress)
}

fn finish_chain(
    agent: &ureq::Agent,
    ms: &MsTokenResponse,
    progress: impl Fn(ChainStage),
) -> Result<MinecraftAccount, AuthError> {
    progress(ChainStage::AuthenticatingXboxLive);
    let xbl = xbl_authenticate(agent, &ms.access_token)?;
    let uhs = xbl
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or_else(|| AuthError::Other("XBL response missing uhs".into()))?;

    progress(ChainStage::AuthorizingXsts);
    let xsts = xsts_authorize(agent, &xbl.token)?;

    progress(ChainStage::LoggingIntoMinecraft);
    let mc = mc_login(agent, &xsts.token, &uhs)?;

    progress(ChainStage::FetchingProfile);
    let profile = mc_profile(agent, &mc.access_token)?;

    Ok(MinecraftAccount {
        name: profile.name,
        uuid: profile.id,
        minecraft_token: mc.access_token,
        // Refresh tokens roll on each refresh — prefer the new one if the
        // server gave us one, otherwise re-use the input refresh token.
        ms_refresh_token: ms.refresh_token.clone().unwrap_or_default(),
    })
}

/// Stages emitted during the chain. Mapped to user-facing labels in the
/// settings UI by the service layer.
#[derive(Copy, Clone, Debug)]
pub enum ChainStage {
    OpeningBrowser,
    WaitingForRedirect,
    ExchangingAuthCode,
    RefreshingMicrosoft,
    AuthenticatingXboxLive,
    AuthorizingXsts,
    LoggingIntoMinecraft,
    FetchingProfile,
}

impl ChainStage {
    pub fn label(self) -> &'static str {
        match self {
            ChainStage::OpeningBrowser => "opening browser…",
            ChainStage::WaitingForRedirect => "waiting for sign-in…",
            ChainStage::ExchangingAuthCode => "verifying with Microsoft…",
            ChainStage::RefreshingMicrosoft => "refreshing your session…",
            ChainStage::AuthenticatingXboxLive => "checking Xbox Live…",
            ChainStage::AuthorizingXsts => "authorizing…",
            ChainStage::LoggingIntoMinecraft => "signing into Minecraft…",
            ChainStage::FetchingProfile => "fetching profile…",
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// HTTP plumbing
// ────────────────────────────────────────────────────────────────────────

fn post_form_json<T: serde::de::DeserializeOwned>(
    agent: &ureq::Agent,
    url: &str,
    body: &str,
) -> Result<T, AuthError> {
    let res = agent
        .post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .send_string(body);
    match res {
        Ok(r) => r
            .into_json()
            .map_err(|e| AuthError::Other(format!("body parse: {}", e))),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(AuthError::Other(format!("{} {}: {}", url, code, body)))
        }
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

fn post_json<T: serde::de::DeserializeOwned>(
    agent: &ureq::Agent,
    url: &str,
    body: &serde_json::Value,
) -> Result<T, AuthError> {
    let res = agent
        .post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_json(body);
    match res {
        Ok(r) => r
            .into_json()
            .map_err(|e| AuthError::Other(format!("body parse: {}", e))),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(AuthError::Other(format!("{} {}: {}", url, code, body)))
        }
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}
