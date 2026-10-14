//! Linux secret storage: the desktop's Secret Service (GNOME Keyring, KWallet,
//! KeePassXC, ...) through the `keyring` crate's **blocking** D-Bus backend
//! (`sync-secret-service`; no async runtime, per the project's rules).
//!
//! It mirrors the Windows DPAPI design (see [`super::secret`]):
//!
//! - **Seal on save.** The secret itself goes into the keyring under a stable
//!   slot name (`<account uuid>:ms_refresh_token`, service `EwoClient`);
//!   `auth.toml` keeps only a `keyring:v1:<base64 slot>` marker.
//! - **Migrate on load.** A legacy plaintext value opens as plain, reports
//!   that it needs resealing, and the next save moves it into the keyring.
//! - **An unopenable entry signs out only that account.** A marker whose
//!   entry is missing, locked or unreadable is an `Err` for that one value.
//! - **No Secret Service, no problem.** If the keyring can't be reached at
//!   startup, the launcher logs a warning and keeps the previous behaviour:
//!   tokens in the `0600` `auth.toml`.
//!
//! The code is generic over keyring's `CredentialBuilder`, so the unit tests
//! drive it with the crate's in-memory mock store on every host; only
//! [`linux_cipher`] names the real backend.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use keyring::credential::CredentialBuilder;
use keyring::{Entry, Error};

use super::secret::{SecretCipher, KEYRING_PREFIX};

/// The keyring "service" every EwoClient entry is filed under.
const SERVICE: &str = "EwoClient";

/// A throwaway slot used only to find out whether the keyring is reachable.
const PROBE_SLOT: &str = "availability-probe";

pub struct KeyringCipher {
    builder: Box<CredentialBuilder>,
    /// One `Entry` per slot, created lazily and reused (a handle, not a
    /// connection; the mock store needs the reuse to remember anything).
    entries: Mutex<HashMap<String, Entry>>,
    /// Slots this process has stored or read successfully: the only ones
    /// worth a (D-Bus) delete when a field is cleared.
    known: Mutex<HashSet<String>>,
}

impl KeyringCipher {
    pub fn new(builder: Box<CredentialBuilder>) -> Self {
        Self {
            builder,
            entries: Mutex::new(HashMap::new()),
            known: Mutex::new(HashSet::new()),
        }
    }

    /// A cipher over `builder`, or why the keyring is unusable here.
    pub fn probe(builder: Box<CredentialBuilder>) -> Result<Self, String> {
        let cipher = Self::new(builder);
        let found = cipher.with_entry(PROBE_SLOT, |e| e.get_password());
        classify_probe(found)?;
        Ok(cipher)
    }

    /// Run `f` on the (cached) entry for `slot`.
    fn with_entry<T>(
        &self,
        slot: &str,
        f: impl FnOnce(&Entry) -> keyring::Result<T>,
    ) -> keyring::Result<T> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if !entries.contains_key(slot) {
            let credential = self.builder.build(None, SERVICE, slot)?;
            entries.insert(slot.to_string(), Entry::new_with_credential(credential));
        }
        let entry = entries.get(slot).expect("inserted above");
        f(entry)
    }

    fn mark_known(&self, slot: &str) {
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(slot.to_string());
    }
}

/// Whether a probe read says the keyring works. A missing entry is the
/// normal answer from a working store; anything else (no D-Bus session, no
/// service, a locked collection that can't be opened) means it doesn't.
fn classify_probe(result: keyring::Result<String>) -> Result<(), String> {
    match result {
        Ok(_) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

impl SecretCipher for KeyringCipher {
    fn encrypts(&self) -> bool {
        true
    }

    fn prefix(&self) -> &'static str {
        KEYRING_PREFIX
    }

    /// Store `plain` in the keyring under `slot`; the "ciphertext" kept in
    /// `auth.toml` is just the slot name, so a marker can't be replayed
    /// against another account's entry.
    fn protect(&self, slot: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
        let text = std::str::from_utf8(plain).map_err(|_| "secret is not UTF-8".to_string())?;
        self.with_entry(slot, |e| e.set_password(text))
            .map_err(|e| format!("keyring: could not store {slot}: {e}"))?;
        self.mark_known(slot);
        Ok(slot.as_bytes().to_vec())
    }

    fn unprotect(&self, slot: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
        if sealed != slot.as_bytes() {
            return Err("keyring marker belongs to a different slot".into());
        }
        match self.with_entry(slot, |e| e.get_password()) {
            Ok(secret) => {
                self.mark_known(slot);
                Ok(secret.into_bytes())
            }
            Err(Error::NoEntry) => Err(format!(
                "no keyring entry for {slot} (removed, or written on another machine)"
            )),
            Err(e) => Err(format!("keyring: could not read {slot}: {e}")),
        }
    }

    fn discard(&self, slot: &str) {
        let was_known = self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(slot);
        if !was_known {
            return;
        }
        match self.with_entry(slot, |e| e.delete_credential()) {
            Ok(()) | Err(Error::NoEntry) => {}
            Err(e) => log::warn!("keyring: could not delete {slot}: {e}"),
        }
    }
}

/// The cipher for Linux: the Secret Service if it answers, else plaintext in
/// the `0600` file (logged once).
#[cfg(target_os = "linux")]
pub fn linux_cipher() -> &'static dyn SecretCipher {
    use std::sync::OnceLock;

    static CIPHER: OnceLock<Box<dyn SecretCipher + Send + Sync>> = OnceLock::new();
    CIPHER
        .get_or_init(|| {
            let chosen: Box<dyn SecretCipher + Send + Sync> =
                match KeyringCipher::probe(keyring::default::default_credential_builder()) {
                    Ok(cipher) => {
                        log::info!(
                            "auth: Secret Service available — tokens are kept in the keyring"
                        );
                        Box::new(cipher)
                    }
                    Err(why) => {
                        log::warn!(
                            "auth: no usable Secret Service ({why}) — refresh tokens stay in \
                             the 0600 auth.toml"
                        );
                        Box::new(super::secret::Plaintext)
                    }
                };
            chosen
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::persistence::{open_secrets, sealed_copy, AccountStore};
    use crate::auth::secret::{self, Opened};
    use crate::auth::MinecraftAccount;
    use keyring::mock::{self, MockCredential};

    fn mock_cipher() -> KeyringCipher {
        KeyringCipher::new(mock::default_credential_builder())
    }

    fn account(name: &str, uuid: &str, social: Option<&str>) -> MinecraftAccount {
        MinecraftAccount {
            name: name.into(),
            uuid: uuid.into(),
            minecraft_token: String::new(),
            minecraft_token_expires_at: None,
            ms_refresh_token: format!("refresh-{uuid}"),
            social_token: social.map(str::to_string),
        }
    }

    fn store() -> AccountStore {
        AccountStore {
            active: Some("uuid-a".into()),
            accounts: vec![
                account("A", "uuid-a", Some("social-a")),
                account("B", "uuid-b", None),
            ],
        }
    }

    const REFRESH_A: &str = "uuid-a:ms_refresh_token";

    /// Make the next call on `slot`'s entry fail with `err`.
    fn fail_next(c: &KeyringCipher, slot: &str, err: Error) {
        c.with_entry(slot, |e| {
            let m: &MockCredential = e.get_credential().downcast_ref().unwrap();
            m.set_error(err);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn seal_puts_the_secret_in_the_keyring_and_only_a_marker_in_the_file() {
        let c = mock_cipher();
        let sealed = secret::seal(&c, REFRESH_A, "the-refresh-token").unwrap();
        assert!(sealed.starts_with(KEYRING_PREFIX), "{sealed}");
        assert!(!sealed.contains("the-refresh-token"));
        assert_eq!(
            secret::open(&c, REFRESH_A, &sealed).unwrap(),
            Opened::Sealed("the-refresh-token".into())
        );
        // Sealing again replaces the entry rather than adding a second one.
        let sealed2 = secret::seal(&c, REFRESH_A, "rotated").unwrap();
        assert_eq!(sealed2, sealed, "the marker is stable per slot");
        assert_eq!(
            secret::open(&c, REFRESH_A, &sealed2).unwrap().into_inner(),
            "rotated"
        );
    }

    #[test]
    fn a_whole_store_round_trips_and_the_file_text_has_no_secrets() {
        let c = mock_cipher();
        let sealed = sealed_copy(&store(), &c).unwrap();
        let text = toml::to_string_pretty(&sealed).unwrap();
        assert!(
            !text.contains("refresh-uuid") && !text.contains("social-a"),
            "{text}"
        );
        assert!(text.contains(KEYRING_PREFIX));
        let mut back = sealed;
        assert!(!open_secrets(&mut back, &c), "sealed values need no reseal");
        assert_eq!(back.accounts[0].ms_refresh_token, "refresh-uuid-a");
        assert_eq!(back.accounts[0].social_token.as_deref(), Some("social-a"));
        assert_eq!(back.accounts[1].ms_refresh_token, "refresh-uuid-b");
        assert_eq!(back.accounts[1].social_token, None);
    }

    #[test]
    fn legacy_plaintext_loads_then_moves_into_the_keyring_on_the_next_save() {
        let c = mock_cipher();
        let mut loaded = store(); // plaintext, as an old 0600 auth.toml holds it
        assert!(open_secrets(&mut loaded, &c), "plaintext asks for a reseal");
        assert_eq!(loaded.accounts[0].ms_refresh_token, "refresh-uuid-a");
        let resealed = sealed_copy(&loaded, &c).unwrap();
        assert!(resealed.accounts[0]
            .ms_refresh_token
            .starts_with(KEYRING_PREFIX));
        assert!(resealed.accounts[1]
            .ms_refresh_token
            .starts_with(KEYRING_PREFIX));
        // ...and it is really there.
        let mut again = resealed;
        open_secrets(&mut again, &c);
        assert_eq!(again.accounts[1].ms_refresh_token, "refresh-uuid-b");
    }

    #[test]
    fn an_unopenable_entry_signs_out_only_that_account() {
        let writer = mock_cipher();
        let sealed = sealed_copy(&store(), &writer).unwrap();
        // A different keyring (another machine / a wiped store) that only
        // knows B's refresh token.
        let reader = mock_cipher();
        secret::seal(&reader, "uuid-b:ms_refresh_token", "refresh-uuid-b").unwrap();
        let mut loaded = sealed;
        open_secrets(&mut loaded, &reader);
        assert_eq!(loaded.accounts[0].ms_refresh_token, "", "A is signed out");
        assert_eq!(
            loaded.accounts[0].social_token, None,
            "A's social token too"
        );
        assert_eq!(loaded.accounts.len(), 2, "no account removed");
        assert_eq!(loaded.accounts[1].ms_refresh_token, "refresh-uuid-b");
    }

    #[test]
    fn a_marker_cannot_be_replayed_against_another_slot() {
        let c = mock_cipher();
        let a = secret::seal(&c, REFRESH_A, "tok-a").unwrap();
        secret::seal(&c, "uuid-b:ms_refresh_token", "tok-b").unwrap();
        assert!(secret::open(&c, "uuid-b:ms_refresh_token", &a).is_err());
    }

    #[test]
    fn clearing_a_field_or_removing_an_account_deletes_its_entry() {
        let c = mock_cipher();
        let sealed = sealed_copy(&store(), &c).unwrap();
        let social_marker = sealed.accounts[0].social_token.clone().unwrap();
        let social_slot = "uuid-a:social_token";
        assert!(secret::open(&c, social_slot, &social_marker).is_ok());

        // Unlinking the social token (None on the next save) removes it.
        let mut unlinked = store();
        unlinked.accounts[0].social_token = None;
        sealed_copy(&unlinked, &c).unwrap();
        assert!(secret::open(&c, social_slot, &social_marker).is_err());

        // Removing the account drops its refresh token entry.
        let refresh_marker = sealed.accounts[0].ms_refresh_token.clone();
        assert!(secret::open(&c, REFRESH_A, &refresh_marker).is_ok());
        c.discard(REFRESH_A);
        assert!(secret::open(&c, REFRESH_A, &refresh_marker).is_err());
    }

    #[test]
    fn a_slot_never_seen_this_session_costs_no_keyring_call() {
        let c = mock_cipher();
        // Would fail loudly if discard reached the entry.
        fail_next(
            &c,
            "ghost:social_token",
            Error::Invalid("x".into(), "y".into()),
        );
        c.discard("ghost:social_token");
        assert!(
            c.with_entry("ghost:social_token", |e| e.get_password())
                .is_err_and(|e| matches!(e, Error::Invalid(..))),
            "the injected error is still pending, so discard never touched the entry"
        );
    }

    #[test]
    fn a_keyring_write_failure_fails_the_seal_instead_of_writing_plaintext() {
        let c = mock_cipher();
        fail_next(&c, REFRESH_A, Error::NoStorageAccess("locked".into()));
        let err = sealed_copy(&store(), &c).unwrap_err();
        assert!(err.contains("could not store"), "{err}");
    }

    #[test]
    fn a_keyring_read_failure_is_an_err_for_that_value_only() {
        let c = mock_cipher();
        let sealed = sealed_copy(&store(), &c).unwrap();
        fail_next(&c, REFRESH_A, Error::NoStorageAccess("locked".into()));
        let mut loaded = sealed;
        open_secrets(&mut loaded, &c);
        assert_eq!(loaded.accounts[0].ms_refresh_token, "");
        assert_eq!(loaded.accounts[1].ms_refresh_token, "refresh-uuid-b");
    }

    #[test]
    fn the_availability_probe_accepts_a_working_store_and_rejects_a_dead_one() {
        assert!(KeyringCipher::probe(mock::default_credential_builder()).is_ok());
        let boom: Box<dyn std::error::Error + Send + Sync> = "no session bus".into();
        assert!(classify_probe(Err(Error::NoEntry)).is_ok());
        assert!(classify_probe(Ok("x".into())).is_ok());
        let why = classify_probe(Err(Error::PlatformFailure(boom))).unwrap_err();
        assert!(why.contains("no session bus"), "{why}");
        assert!(classify_probe(Err(Error::NoStorageAccess("locked".into()))).is_err());
    }
}
