//! PKCE (Proof Key for Code Exchange, RFC 7636) helpers.
//!
//! Used for the Microsoft OAuth Authorization Code flow on a public/native
//! client (no client secret). The flow:
//!
//! 1. Client generates a `code_verifier` — a random URL-safe string.
//! 2. Client derives `code_challenge = base64url(sha256(code_verifier))`.
//! 3. Client opens the auth URL with `code_challenge` + `code_challenge_method=S256`.
//! 4. After redirect, client POSTs the auth code + the original `code_verifier`.
//! 5. Server hashes verifier, compares to challenge — must match.
//!
//! This protects against a network attacker who could intercept the auth
//! code — without the verifier, the code is useless.

use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};

/// One PKCE pair generated for a single auth attempt. Drop it after the
/// token exchange — verifier is one-time-use.
#[derive(Clone, Debug)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

impl PkcePair {
    /// Generate a fresh PKCE pair. Verifier is 64 URL-safe random chars
    /// (within RFC 7636's 43-128 range, comfortably above the minimum).
    pub fn new() -> Self {
        let verifier = generate_verifier(64);
        let challenge = derive_challenge(&verifier);
        Self {
            verifier,
            challenge,
        }
    }
}

fn generate_verifier(len: usize) -> String {
    // RFC 7636 §4.1: code_verifier = high-entropy ASCII from
    // [A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

fn derive_challenge(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let digest = h.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_is_consistent() {
        let p = PkcePair::new();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        assert_eq!(p.challenge, derive_challenge(&p.verifier));
    }

    #[test]
    fn known_vector_matches_rfc() {
        // RFC 7636 §A.1 example
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let c = derive_challenge(v);
        assert_eq!(c, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
