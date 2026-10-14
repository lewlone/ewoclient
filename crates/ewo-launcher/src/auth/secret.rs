//! At-rest protection for the credentials in `auth.toml`.
//!
//! On Windows every secret (MS refresh token, social token) is sealed with
//! DPAPI (`CryptProtectData`, current-user scope, `CRYPTPROTECT_UI_FORBIDDEN`)
//! and stored as `dpapi:v1:<base64>`. Only the same Windows user on the same
//! machine can open it.
//!
//! On Linux the secrets live in the desktop's Secret Service (GNOME Keyring,
//! KWallet, ...) through the `keyring` crate: `auth.toml` then holds only a
//! `keyring:v1:<base64 slot name>` marker per secret ([`super::keyring_cipher`]).
//! With no Secret Service running, and on other platforms, the value stays
//! plaintext: the file is already written `0600` by
//! [`crate::util::atomic_write_private`].
//!
//! The cipher is behind [`SecretCipher`] so the seal/open/migration rules
//! are testable on any platform with a fake. Every secret has a stable
//! **slot** name (see [`slot`]) so a store-backed cipher can find, replace and
//! delete its entry; ciphers that embed the ciphertext in the file ignore it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Marks a DPAPI-sealed value. The version lets a future scheme coexist.
pub const PREFIX: &str = "dpapi:v1:";

/// Marks a value that lives in the OS keyring (Secret Service).
pub const KEYRING_PREFIX: &str = "keyring:v1:";

/// Every prefix some cipher writes. A value with one of these that the
/// current cipher doesn't own can't be opened here.
const KNOWN_PREFIXES: [&str; 2] = [PREFIX, KEYRING_PREFIX];

/// The stable name of one account's secret (`kind` is the field name).
pub fn slot(account_uuid: &str, kind: &str) -> String {
    format!("{account_uuid}:{kind}")
}

/// The slot kinds an account has.
pub const KIND_REFRESH: &str = "ms_refresh_token";
pub const KIND_SOCIAL: &str = "social_token";

/// A reversible, machine/user-bound secret store.
pub trait SecretCipher {
    /// `true` when [`protect`](Self::protect) really protects. `false` for
    /// the plaintext fallback, which then never writes a sealed value.
    fn encrypts(&self) -> bool;
    /// The prefix this cipher's sealed values carry.
    fn prefix(&self) -> &'static str {
        PREFIX
    }
    fn protect(&self, slot: &str, plain: &[u8]) -> Result<Vec<u8>, String>;
    fn unprotect(&self, slot: &str, sealed: &[u8]) -> Result<Vec<u8>, String>;
    /// Drop whatever is stored for `slot` (a signed-out field, a removed
    /// account). Ciphers that embed the ciphertext in the file have nothing
    /// to drop.
    fn discard(&self, _slot: &str) {}
}

/// A stored value, opened.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// Was sealed on disk.
    Sealed(String),
    /// Was plaintext on disk (pre-DPAPI file, or a non-Windows platform).
    Plain(String),
}

impl Opened {
    pub fn into_inner(self) -> String {
        match self {
            Opened::Sealed(s) | Opened::Plain(s) => s,
        }
    }
}

/// Seal `plain` for storage. An empty value stays `""` (and drops whatever
/// the cipher kept for `slot`, so a signed-out field leaves nothing behind);
/// the plaintext cipher passes values through unchanged.
pub fn seal(cipher: &dyn SecretCipher, slot: &str, plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        cipher.discard(slot);
        return Ok(String::new());
    }
    if !cipher.encrypts() {
        return Ok(plain.to_string());
    }
    let sealed = cipher.protect(slot, plain.as_bytes())?;
    Ok(format!("{}{}", cipher.prefix(), STANDARD.encode(sealed)))
}

/// Open a stored value. An unprefixed value is legacy plaintext and is
/// returned as-is; a prefixed one that won't decode or decrypt (another
/// user/machine, corrupted, sealed by a scheme this platform doesn't have, or
/// missing from the keyring) is an `Err`.
pub fn open(cipher: &dyn SecretCipher, slot: &str, stored: &str) -> Result<Opened, String> {
    let Some(b64) = stored.strip_prefix(cipher.prefix()) else {
        return match KNOWN_PREFIXES.iter().find(|p| stored.starts_with(**p)) {
            Some(p) => Err(format!(
                "value is sealed with \"{p}\", which is unavailable here"
            )),
            None => Ok(Opened::Plain(stored.to_string())),
        };
    };
    if !cipher.encrypts() {
        return Err("value is sealed, but this platform's cipher cannot open it".into());
    }
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("bad base64: {e}"))?;
    let plain = cipher.unprotect(slot, &bytes)?;
    String::from_utf8(plain)
        .map(Opened::Sealed)
        .map_err(|_| "decrypted value is not UTF-8".to_string())
}

/// Stores secrets as-is. The platform cipher where nothing better exists.
#[cfg_attr(windows, allow(dead_code))]
pub struct Plaintext;

impl SecretCipher for Plaintext {
    fn encrypts(&self) -> bool {
        false
    }
    fn protect(&self, _slot: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }
    fn unprotect(&self, _slot: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
        Ok(sealed.to_vec())
    }
}

#[cfg(windows)]
pub use dpapi::Dpapi;

#[cfg(windows)]
mod dpapi {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    use super::SecretCipher;

    /// Windows DPAPI, current-user scope.
    pub struct Dpapi;

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// Copy DPAPI's output out and free it with `LocalFree`, as the API
    /// requires.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as *mut _));
        v
    }

    impl SecretCipher for Dpapi {
        fn encrypts(&self) -> bool {
            true
        }

        fn protect(&self, _slot: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
            let input = blob(plain);
            let mut out = CRYPT_INTEGER_BLOB::default();
            unsafe {
                CryptProtectData(
                    &input,
                    w!("EwoClient"),
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
                .map_err(|e| format!("CryptProtectData: {e}"))?;
                Ok(take(out))
            }
        }

        fn unprotect(&self, _slot: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
            let input = blob(sealed);
            let mut out = CRYPT_INTEGER_BLOB::default();
            unsafe {
                CryptUnprotectData(
                    &input,
                    None,
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
                .map_err(|e| format!("CryptUnprotectData: {e}"))?;
                Ok(take(out))
            }
        }
    }
}

/// The cipher this platform persists with.
pub fn platform_cipher() -> &'static dyn SecretCipher {
    #[cfg(windows)]
    {
        &Dpapi
    }
    #[cfg(target_os = "linux")]
    {
        super::keyring_cipher::linux_cipher()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        &Plaintext
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// XORs every byte; fails to open anything not produced by itself
    /// (a trailing tag byte stands in for DPAPI's integrity check).
    pub(crate) struct FakeCipher;

    impl SecretCipher for FakeCipher {
        fn encrypts(&self) -> bool {
            true
        }
        fn protect(&self, _slot: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
            let mut v: Vec<u8> = plain.iter().map(|b| b ^ 0x5A).collect();
            v.push(0xEE);
            Ok(v)
        }
        fn unprotect(&self, _slot: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
            match sealed.split_last() {
                Some((0xEE, body)) => Ok(body.iter().map(|b| b ^ 0x5A).collect()),
                _ => Err("integrity check failed".into()),
            }
        }
    }

    const SLOT: &str = "uuid-a:ms_refresh_token";

    #[test]
    fn seal_then_open_round_trips() {
        let sealed = seal(&FakeCipher, SLOT, "refresh-tok").unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("refresh-tok"));
        assert_eq!(
            open(&FakeCipher, SLOT, &sealed).unwrap(),
            Opened::Sealed("refresh-tok".into())
        );
    }

    #[test]
    fn unprefixed_value_is_legacy_plaintext() {
        assert_eq!(
            open(&FakeCipher, SLOT, "old-tok").unwrap(),
            Opened::Plain("old-tok".into())
        );
    }

    #[test]
    fn empty_value_stays_empty() {
        assert_eq!(seal(&FakeCipher, SLOT, "").unwrap(), "");
    }

    #[test]
    fn foreign_or_corrupt_sealed_value_errors() {
        // Valid base64, but not produced by this cipher (another user/machine).
        let foreign = format!("{PREFIX}{}", STANDARD.encode([1u8, 2, 3]));
        assert!(open(&FakeCipher, SLOT, &foreign).is_err());
        assert!(open(&FakeCipher, SLOT, &format!("{PREFIX}!!not base64!!")).is_err());
    }

    #[test]
    fn plaintext_cipher_never_seals_and_cannot_open_sealed() {
        assert_eq!(seal(&Plaintext, SLOT, "tok").unwrap(), "tok");
        let sealed = seal(&FakeCipher, SLOT, "tok").unwrap();
        assert!(open(&Plaintext, SLOT, &sealed).is_err());
        // Nor a keyring marker, whose entry this cipher can't reach.
        let marker = format!("{KEYRING_PREFIX}{}", STANDARD.encode(SLOT));
        assert!(open(&Plaintext, SLOT, &marker).is_err());
        assert!(open(&FakeCipher, SLOT, &marker).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips() {
        let sealed = seal(&Dpapi, SLOT, "ms-refresh-\u{e9}-token").unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert_eq!(
            open(&Dpapi, SLOT, &sealed).unwrap().into_inner(),
            "ms-refresh-\u{e9}-token"
        );
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_tampered_ciphertext() {
        let sealed = seal(&Dpapi, SLOT, "tok").unwrap();
        let mut bytes = STANDARD.decode(&sealed[PREFIX.len()..]).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let tampered = format!("{PREFIX}{}", STANDARD.encode(bytes));
        assert!(open(&Dpapi, SLOT, &tampered).is_err());
    }
}
