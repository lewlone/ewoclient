//! On-disk auth state at `<config>/EwoClient/auth.toml`.
//!
//! Holds an [`AccountStore`] — the list of signed-in Microsoft accounts plus
//! a pointer to the active one (Phase F). A pre-F `auth.toml` held a single
//! `account`; [`load_store`] migrates that v1 schema transparently and
//! rewrites the file as v2.
//!
//! Refresh tokens are stored in plaintext. **They are credentials** —
//! encrypting at rest is a follow-up (DPAPI on Windows, libsecret/keychain
//! on Linux). For the single-user dev target it's acceptable; the file
//! lives in the user's own config directory, the same trust boundary as
//! their browser cookies.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::MinecraftAccount;

const FILENAME: &str = "auth.toml";

/// Current `auth.toml` schema version. v1 = pre-F single-account; v2 = the
/// Phase F account store.
const CURRENT_VERSION: u32 = 2;

/// The set of signed-in accounts plus which one is active. Serialized as
/// the `[store]` table of `auth.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountStore {
    /// UUID of the active account — the one launches use. `None` when no
    /// account is signed in, or the active one was removed. Declared
    /// before `accounts` so it serializes ahead of the array-of-tables.
    #[serde(default)]
    pub active: Option<String>,
    /// All signed-in accounts. The persisted form skips the short-lived
    /// `minecraft_token` (it's `#[serde(skip)]` on `MinecraftAccount`).
    #[serde(default)]
    pub accounts: Vec<MinecraftAccount>,
}

impl AccountStore {
    /// The active account, if the `active` pointer names a present account.
    pub fn active_account(&self) -> Option<&MinecraftAccount> {
        let uuid = self.active.as_ref()?;
        self.accounts.iter().find(|a| &a.uuid == uuid)
    }

    /// Insert `account`, or replace the existing entry with the same UUID
    /// (e.g. after a silent refresh produces a fresh refresh token). Does
    /// not change the `active` pointer.
    pub fn upsert(&mut self, account: MinecraftAccount) {
        if let Some(slot) = self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            *slot = account;
        } else {
            self.accounts.push(account);
        }
    }

    /// Remove the account with `uuid`. If it was the active one, the active
    /// pointer falls back to the first remaining account, or `None` if the
    /// store is now empty.
    pub fn remove(&mut self, uuid: &str) {
        self.accounts.retain(|a| a.uuid != uuid);
        if self.active.as_deref() == Some(uuid) {
            self.active = self.accounts.first().map(|a| a.uuid.clone());
        }
    }
}

/// On-disk wrapper. `version` disambiguates the schema; `store` is the v2
/// payload; `account` is the v1 legacy field, kept read-only so a pre-F
/// file still parses.
#[derive(Debug, Serialize, Deserialize)]
struct AuthFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    store: AccountStore,
    /// v1 legacy single-account field. Read-only — present so a pre-F
    /// `auth.toml` still parses; F-era code never writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// Parse `auth.toml` contents into an `AccountStore`. Returns the store and
/// whether a v1 file was migrated (so the caller can rewrite it as v2).
/// `None` for an unparseable or unknown-version file — callers treat that
/// as an empty store and the user re-authenticates.
fn parse_store(s: &str) -> Option<(AccountStore, bool)> {
    let file: AuthFile = toml::from_str(s).ok()?;
    match file.version {
        // Versionless (`default_version` = 1) or v1 — the pre-F schema.
        1 => {
            let mut store = AccountStore::default();
            if let Some(account) = file.account {
                let uuid = account.uuid.clone();
                store.accounts.push(account);
                store.active = Some(uuid);
            }
            Some((store, true))
        }
        2 => Some((file.store, false)),
        other => {
            log::warn!("auth: auth.toml has unknown version {other} — ignoring");
            None
        }
    }
}

/// Load the account store, migrating a pre-F single-account file in place.
/// Never fails — any error (missing, malformed, unknown version) yields an
/// empty store and the user signs in fresh.
pub fn load_store() -> AccountStore {
    let Some(path) = auth_path() else {
        return AccountStore::default();
    };
    if !path.exists() {
        return AccountStore::default();
    }
    let contents = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("auth: read {} failed: {}", path.display(), e);
            return AccountStore::default();
        }
    };
    match parse_store(&contents) {
        Some((store, migrated)) => {
            log::info!(
                "auth: loaded {} account(s) from {}",
                store.accounts.len(),
                path.display(),
            );
            if migrated {
                log::info!("auth: migrated auth.toml v1 -> v2");
                save_store(&store);
            }
            store
        }
        None => AccountStore::default(),
    }
}

/// Persist the account store. Best-effort — failures log a warning but
/// don't surface to the user (worst case, they re-sign-in next launch).
pub fn save_store(store: &AccountStore) {
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
        version: CURRENT_VERSION,
        store: store.clone(),
        account: None,
    };
    match toml::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                log::warn!("auth: write {} failed: {}", path.display(), e);
            } else {
                log::info!("auth: saved {} account(s)", store.accounts.len());
            }
        }
        Err(e) => log::warn!("auth: serialize failed: {}", e),
    }
}

/// Convenience: the active account, if any. Keeps the single-account
/// startup path (`App` hydrates + silent-refreshes the active account)
/// unchanged while the store is plural underneath.
pub fn load() -> Option<MinecraftAccount> {
    load_store().active_account().cloned()
}

/// Convenience: record `account` as signed-in and active. Upserts it into
/// the store (replacing a stale entry with the same UUID, e.g. after a
/// silent refresh) and persists.
pub fn save(account: &MinecraftAccount) {
    let mut store = load_store();
    store.upsert(account.clone());
    store.active = Some(account.uuid.clone());
    save_store(&store);
}

/// Convenience: sign out the active account — remove it from the store and
/// persist. Other accounts (Phase F) are left intact.
pub fn clear() {
    let mut store = load_store();
    let Some(active) = store.active.clone() else {
        return;
    };
    store.remove(&active);
    save_store(&store);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str, uuid: &str) -> MinecraftAccount {
        MinecraftAccount {
            name: name.to_string(),
            uuid: uuid.to_string(),
            minecraft_token: String::new(),
            ms_refresh_token: format!("refresh-{uuid}"),
        }
    }

    #[test]
    fn v1_single_account_migrates_to_active_store() {
        let v1 = r#"
            version = 1
            [account]
            name = "Vwyla"
            uuid = "uuid-a"
            ms_refresh_token = "tok-a"
        "#;
        let (store, migrated) = parse_store(v1).expect("v1 parses");
        assert!(migrated, "v1 file should report migrated");
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.active.as_deref(), Some("uuid-a"));
        assert_eq!(store.active_account().unwrap().name, "Vwyla");
    }

    #[test]
    fn versionless_file_treated_as_v1() {
        let legacy = r#"
            [account]
            name = "Old"
            uuid = "uuid-x"
            ms_refresh_token = "tok-x"
        "#;
        let (store, migrated) = parse_store(legacy).expect("versionless parses");
        assert!(migrated);
        assert_eq!(store.active.as_deref(), Some("uuid-x"));
    }

    #[test]
    fn v2_store_round_trips_through_toml() {
        let mut store = AccountStore::default();
        store.upsert(account("One", "uuid-1"));
        store.upsert(account("Two", "uuid-2"));
        store.active = Some("uuid-2".to_string());

        let file = AuthFile {
            version: CURRENT_VERSION,
            store: store.clone(),
            account: None,
        };
        let toml_text = toml::to_string_pretty(&file).expect("serialize");

        let (parsed, migrated) = parse_store(&toml_text).expect("v2 parses");
        assert!(!migrated, "v2 file is not a migration");
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.active.as_deref(), Some("uuid-2"));
        assert_eq!(parsed.active_account().unwrap().name, "Two");
    }

    #[test]
    fn unknown_version_yields_none() {
        assert!(parse_store("version = 99\n").is_none());
    }

    #[test]
    fn upsert_replaces_entry_with_same_uuid() {
        let mut store = AccountStore::default();
        store.upsert(account("Before", "uuid-1"));
        store.upsert(account("After", "uuid-1"));
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].name, "After");
    }

    #[test]
    fn removing_active_account_repoints_to_first_remaining() {
        let mut store = AccountStore::default();
        store.upsert(account("One", "uuid-1"));
        store.upsert(account("Two", "uuid-2"));
        store.active = Some("uuid-2".to_string());

        store.remove("uuid-2");
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.active.as_deref(), Some("uuid-1"), "active falls back");

        store.remove("uuid-1");
        assert!(store.active.is_none(), "empty store has no active");
    }
}
