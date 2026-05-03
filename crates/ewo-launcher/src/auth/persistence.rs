//! On-disk auth state at `<config>/EwoClient/auth.toml`.
//!
//! Currently stores the Microsoft refresh token in plaintext. **This is a
//! credential** — encrypting at rest is a follow-up if/when we ship this
//! binary widely (DPAPI on Windows, libsecret/keychain on Linux). For the
//! single-user dev target it's acceptable; the file lives in the user's
//! own config directory and is the same trust boundary as their browser
//! cookies.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::MinecraftAccount;

const FILENAME: &str = "auth.toml";

#[derive(Debug, Serialize, Deserialize)]
struct AuthFile {
    /// Schema version. Bump if the format changes incompatibly so we can
    /// gracefully fall back to "signed out" instead of crashing on parse.
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    account: Option<MinecraftAccount>,
}

fn default_version() -> u32 {
    1
}

fn auth_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(FILENAME);
    Some(p)
}

/// Load the persisted account, if any. Returns `None` for any failure
/// mode (file missing, malformed, version mismatch) — callers fall back
/// to signed-out and the user re-authenticates.
pub fn load() -> Option<MinecraftAccount> {
    let path = auth_path()?;
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<AuthFile>(&s) {
            Ok(file) if file.version == 1 => {
                if file.account.is_some() {
                    log::info!("auth: loaded persisted account from {}", path.display());
                }
                file.account
            }
            Ok(file) => {
                log::warn!(
                    "auth: {} has unknown version {} — ignoring",
                    path.display(),
                    file.version,
                );
                None
            }
            Err(e) => {
                log::warn!("auth: parse {} failed: {} — signing out", path.display(), e);
                None
            }
        },
        Err(e) => {
            log::warn!("auth: read {} failed: {}", path.display(), e);
            None
        }
    }
}

/// Persist the current account. Best-effort — failures log a warning
/// but don't surface to the user (worst case, they re-sign-in next launch).
pub fn save(account: &MinecraftAccount) {
    let Some(path) = auth_path() else {
        log::warn!("auth: config dir unresolvable — not persisting");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("auth: could not create {}: {}", parent.display(), e);
            return;
        }
    }
    let file = AuthFile {
        version: 1,
        account: Some(account.clone()),
    };
    match toml::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                log::warn!("auth: write {} failed: {}", path.display(), e);
            } else {
                log::info!("auth: saved account to {}", path.display());
            }
        }
        Err(e) => log::warn!("auth: serialize failed: {}", e),
    }
}

/// Wipe the saved account on sign-out.
pub fn clear() {
    let Some(path) = auth_path() else {
        return;
    };
    if path.exists() {
        if let Err(e) = fs::remove_file(&path) {
            log::warn!("auth: remove {} failed: {}", path.display(), e);
        } else {
            log::info!("auth: cleared persisted account");
        }
    }
}
