//! On-disk auth state at `<config>/EwoClient/auth.toml`.
//!
//! Holds an [`AccountStore`] — the list of signed-in Microsoft accounts plus
//! a pointer to the active one (Phase F). A pre-F `auth.toml` held a single
//! `account`; [`load_store`] migrates that v1 schema transparently and
//! rewrites the file as v2.
//!
//! The secrets (`ms_refresh_token`, `social_token`) are sealed at rest by
//! [`super::secret`]: DPAPI on Windows (only this Windows user on this
//! machine can open them), plaintext in a `0600` file elsewhere. A pre-DPAPI
//! plaintext file loads fine and is re-saved sealed.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{secret, MinecraftAccount};

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
            // Preserve Phase H social_token across re-auth (the auth chain
            // doesn't manage it). If the caller explicitly set one, honor
            // that — otherwise carry the existing token forward.
            let preserved = account
                .social_token
                .clone()
                .or_else(|| slot.social_token.clone());
            *slot = account;
            slot.social_token = preserved;
        } else {
            self.accounts.push(account);
        }
    }

    /// Phase H2: persist a social_token against an existing account.
    /// Returns `true` if the account exists and was updated, `false`
    /// if `uuid` doesn't match any known account.
    pub fn set_social_token(&mut self, uuid: &str, token: String) -> bool {
        if let Some(slot) = self.accounts.iter_mut().find(|a| a.uuid == uuid) {
            slot.social_token = Some(token);
            true
        } else {
            false
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
        Some((mut store, migrated)) => {
            let reseal = open_secrets(&mut store, secret::platform_cipher());
            log::info!(
                "auth: loaded {} account(s) from {}",
                store.accounts.len(),
                path.display(),
            );
            if migrated || reseal {
                log::info!("auth: rewriting auth.toml (schema v2, secrets sealed)");
                save_store(&store);
            }
            store
        }
        None => {
            // Keep the unreadable file (it may hold every refresh token) —
            // the next save would otherwise replace it with an empty store.
            crate::util::backup_unparseable(&path, &"unparseable or unknown-version auth.toml");
            AccountStore::default()
        }
    }
}

/// Open every sealed secret in `store`. A secret that cannot be opened (sealed
/// by another user or machine, or corrupted) is dropped, which signs that
/// account out rather than failing the whole load. Returns whether any value
/// was plaintext that this platform would seal, so the caller re-saves it.
fn open_secrets(store: &mut AccountStore, cipher: &dyn secret::SecretCipher) -> bool {
    let mut had_plain = false;
    for a in &mut store.accounts {
        match secret::open(cipher, &a.ms_refresh_token) {
            Ok(opened) => {
                had_plain |= matches!(opened, secret::Opened::Plain(ref v) if !v.is_empty());
                a.ms_refresh_token = opened.into_inner();
            }
            Err(e) => {
                log::warn!("auth: refresh token for {} could not be opened ({e}); sign in again", a.name);
                a.ms_refresh_token.clear();
            }
        }
        if let Some(tok) = a.social_token.take() {
            match secret::open(cipher, &tok) {
                Ok(opened) => {
                    had_plain |= matches!(opened, secret::Opened::Plain(_));
                    a.social_token = Some(opened.into_inner());
                }
                Err(e) => log::warn!("auth: social token for {} dropped ({e}); relink", a.name),
            }
        }
    }
    had_plain && cipher.encrypts()
}

/// A copy of `store` with every secret sealed for writing.
fn sealed_copy(store: &AccountStore, cipher: &dyn secret::SecretCipher) -> Result<AccountStore, String> {
    let mut out = store.clone();
    for a in &mut out.accounts {
        a.ms_refresh_token = secret::seal(cipher, &a.ms_refresh_token)?;
        if let Some(tok) = a.social_token.take() {
            a.social_token = Some(secret::seal(cipher, &tok)?);
        }
    }
    Ok(out)
}

/// Persist the account store. Best-effort — failures log a warning but
/// don't surface to the user (worst case, they re-sign-in next launch).
pub fn save_store(store: &AccountStore) {
    let Some(path) = auth_path() else {
        log::warn!("auth: config dir unresolvable — not persisting");
        return;
    };
    // Never fall back to writing plaintext on a platform that seals: a failed
    // seal keeps the previous file instead.
    let sealed = match sealed_copy(store, secret::platform_cipher()) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("auth: sealing secrets failed ({e}) — not persisting");
            return;
        }
    };
    let file = AuthFile {
        version: CURRENT_VERSION,
        store: sealed,
        account: None,
    };
    match toml::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = crate::util::atomic_write_private(&path, s.as_bytes()) {
                log::warn!("auth: write {} failed: {}", path.display(), e);
            } else {
                log::info!("auth: saved {} account(s)", store.accounts.len());
            }
        }
        Err(e) => log::warn!("auth: serialize failed: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::secret::tests::FakeCipher;
    use crate::auth::secret::{self, Plaintext};

    fn two_accounts() -> AccountStore {
        let mut a = account("A", "uuid-a");
        a.social_token = Some("social-a".into());
        AccountStore { active: Some("uuid-a".into()), accounts: vec![a, account("B", "uuid-b")] }
    }

    #[test]
    fn sealed_copy_hides_secrets_and_opens_back() {
        let store = two_accounts();
        let sealed = sealed_copy(&store, &FakeCipher).unwrap();
        let text = toml::to_string_pretty(&sealed).unwrap();
        assert!(!text.contains("refresh-uuid-a") && !text.contains("social-a"), "{text}");
        let mut back = sealed;
        assert!(!open_secrets(&mut back, &FakeCipher), "sealed values need no reseal");
        assert_eq!(back.accounts[0].ms_refresh_token, "refresh-uuid-a");
        assert_eq!(back.accounts[0].social_token.as_deref(), Some("social-a"));
        assert_eq!(back.accounts[1].ms_refresh_token, "refresh-uuid-b");
    }

    #[test]
    fn legacy_plaintext_loads_and_asks_for_reseal() {
        let mut store = two_accounts();
        assert!(open_secrets(&mut store, &FakeCipher));
        assert_eq!(store.accounts[0].ms_refresh_token, "refresh-uuid-a");
    }

    #[test]
    fn unopenable_secret_signs_out_only_that_account() {
        let mut store = sealed_copy(&two_accounts(), &FakeCipher).unwrap();
        let foreign = format!("{}{}", secret::PREFIX, "AQID"); // valid base64, not ours
        store.accounts[0].ms_refresh_token = foreign.clone();
        store.accounts[0].social_token = Some(foreign);
        open_secrets(&mut store, &FakeCipher);
        assert_eq!(store.accounts[0].ms_refresh_token, "", "signed out");
        assert_eq!(store.accounts[0].social_token, None);
        assert_eq!(store.accounts.len(), 2, "no account removed");
        assert_eq!(store.accounts[1].ms_refresh_token, "refresh-uuid-b");
    }

    #[test]
    fn plaintext_platform_never_forces_a_resave() {
        let mut store = two_accounts();
        assert!(!open_secrets(&mut store, &Plaintext));
        let copy = sealed_copy(&store, &Plaintext).unwrap();
        assert_eq!(copy.accounts[0].ms_refresh_token, "refresh-uuid-a");
    }

    fn account(name: &str, uuid: &str) -> MinecraftAccount {
        MinecraftAccount {
            name: name.to_string(),
            uuid: uuid.to_string(),
            minecraft_token: String::new(),
            ms_refresh_token: format!("refresh-{uuid}"),
            social_token: None,
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
