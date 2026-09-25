//! At-rest protection for the credentials in `auth.toml`.
//!
//! On Windows every secret (MS refresh token, social token) is sealed with
//! DPAPI (`CryptProtectData`, current-user scope, `CRYPTPROTECT_UI_FORBIDDEN`)
//! and stored as `dpapi:v1:<base64>`. Only the same Windows user on the same
//! machine can open it. Elsewhere the value stays plaintext — the file is
//! already written `0600` by [`crate::util::atomic_write_private`].
//!
//! The cipher is behind [`SecretCipher`] so the seal/open/migration rules
//! are testable on any platform with a fake.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Marks a sealed value. The version lets a future scheme coexist.
pub const PREFIX: &str = "dpapi:v1:";

/// A reversible, machine/user-bound byte cipher.
pub trait SecretCipher {
    /// `true` when [`protect`](Self::protect) really encrypts. `false` for
    /// the plaintext fallback, which then never writes a sealed value.
    fn encrypts(&self) -> bool;
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String>;
    fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, String>;
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

/// Seal `plain` for storage. Empty values and the plaintext cipher pass
/// through unchanged, so a signed-out field stays `""` on disk.
pub fn seal(cipher: &dyn SecretCipher, plain: &str) -> Result<String, String> {
    if plain.is_empty() || !cipher.encrypts() {
        return Ok(plain.to_string());
    }
    let sealed = cipher.protect(plain.as_bytes())?;
    Ok(format!("{PREFIX}{}", STANDARD.encode(sealed)))
}

/// Open a stored value. An unprefixed value is legacy plaintext and is
/// returned as-is; a prefixed one that won't decode or decrypt (another
/// user/machine, corrupted, or no DPAPI on this platform) is an `Err`.
pub fn open(cipher: &dyn SecretCipher, stored: &str) -> Result<Opened, String> {
    let Some(b64) = stored.strip_prefix(PREFIX) else {
        return Ok(Opened::Plain(stored.to_string()));
    };
    if !cipher.encrypts() {
        return Err("value is DPAPI-sealed; DPAPI is unavailable on this platform".into());
    }
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("bad base64: {e}"))?;
    let plain = cipher.unprotect(&bytes)?;
    String::from_utf8(plain)
        .map(Opened::Sealed)
        .map_err(|_| "decrypted value is not UTF-8".to_string())
}

/// Stores secrets as-is. The non-Windows platform cipher.
#[cfg_attr(windows, allow(dead_code))]
pub struct Plaintext;

impl SecretCipher for Plaintext {
    fn encrypts(&self) -> bool {
        false
    }
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }
    fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
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

        fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
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

        fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
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
    #[cfg(not(windows))]
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
        fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
            let mut v: Vec<u8> = plain.iter().map(|b| b ^ 0x5A).collect();
            v.push(0xEE);
            Ok(v)
        }
        fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
            match sealed.split_last() {
                Some((0xEE, body)) => Ok(body.iter().map(|b| b ^ 0x5A).collect()),
                _ => Err("integrity check failed".into()),
            }
        }
    }

    #[test]
    fn seal_then_open_round_trips() {
        let sealed = seal(&FakeCipher, "refresh-tok").unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("refresh-tok"));
        assert_eq!(open(&FakeCipher, &sealed).unwrap(), Opened::Sealed("refresh-tok".into()));
    }

    #[test]
    fn unprefixed_value_is_legacy_plaintext() {
        assert_eq!(open(&FakeCipher, "old-tok").unwrap(), Opened::Plain("old-tok".into()));
    }

    #[test]
    fn empty_value_stays_empty() {
        assert_eq!(seal(&FakeCipher, "").unwrap(), "");
    }

    #[test]
    fn foreign_or_corrupt_sealed_value_errors() {
        // Valid base64, but not produced by this cipher (another user/machine).
        let foreign = format!("{PREFIX}{}", STANDARD.encode([1u8, 2, 3]));
        assert!(open(&FakeCipher, &foreign).is_err());
        assert!(open(&FakeCipher, &format!("{PREFIX}!!not base64!!")).is_err());
    }

    #[test]
    fn plaintext_cipher_never_seals_and_cannot_open_sealed() {
        assert_eq!(seal(&Plaintext, "tok").unwrap(), "tok");
        let sealed = seal(&FakeCipher, "tok").unwrap();
        assert!(open(&Plaintext, &sealed).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips() {
        let sealed = seal(&Dpapi, "ms-refresh-\u{e9}-token").unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert_eq!(open(&Dpapi, &sealed).unwrap().into_inner(), "ms-refresh-\u{e9}-token");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_tampered_ciphertext() {
        let sealed = seal(&Dpapi, "tok").unwrap();
        let mut bytes = STANDARD.decode(&sealed[PREFIX.len()..]).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let tampered = format!("{PREFIX}{}", STANDARD.encode(bytes));
        assert!(open(&Dpapi, &tampered).is_err());
    }
}
