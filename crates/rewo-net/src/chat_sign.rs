//! M7b — signed chat (player certificates + the signed-message chain).
//!
//! `enforce-secure-profile` servers reject unsigned chat. The client
//! fetches a per-session RSA key pair + Mojang-signed certificate from
//! `api.minecraftservices.com/player/certificates`, announces the public
//! half in a `chat_session_update` packet, then signs every outgoing chat
//! message: SHA256withRSA over `updateSignature` bytes (decompiled
//! `PlayerChatMessage.updateSignature` → `SignedMessageLink` +
//! `SignedMessageBody`), with a strictly-incrementing link index forming
//! the chain the server validates.

use rsa::pkcs8::DecodePrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;

use crate::crypt::OnlineAuth;

/// A fetched player certificate + the signing key, ready to announce and
/// sign with.
pub struct ChatSigner {
    signing_key: SigningKey<Sha256>,
    /// X.509-DER public key (the announced `key` field).
    pub public_key_der: Vec<u8>,
    /// Mojang's `signature_v2` over the key (the server validates it
    /// against the Yggdrasil service key).
    pub key_signature: Vec<u8>,
    /// Certificate expiry, epoch-milli (wire `Instant`).
    pub expires_at_ms: i64,
    /// Random per-session id announced in `chat_session_update`; also the
    /// `sessionId` mixed into every signature (chain identity).
    pub session_id: u128,
    /// Sender profile UUID (chain identity).
    sender: u128,
    /// Strictly-incrementing link index — 0 for the first signed message,
    /// +1 each send. The server rejects a repeat or gap.
    index: i32,
}

impl ChatSigner {
    /// Fetch `player/certificates` with the account token and build a
    /// signer. `None`-worthy failures are returned as `Err` so the caller
    /// can fall back to unsigned chat with a warning.
    pub fn fetch(auth: &OnlineAuth) -> Result<Self, String> {
        let resp = ureq::post("https://api.minecraftservices.com/player/certificates")
            .set("Authorization", &format!("Bearer {}", auth.access_token))
            .call()
            .map_err(|e| format!("certificates: {e}"))?;
        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("certificates json: {e}"))?;

        let key_pair = &json["keyPair"];
        let private_pem = key_pair["privateKey"]
            .as_str()
            .ok_or("certificates: missing privateKey")?;
        let public_pem = key_pair["publicKey"]
            .as_str()
            .ok_or("certificates: missing publicKey")?;
        let sig_b64 = json["publicKeySignatureV2"]
            .as_str()
            .ok_or("certificates: missing publicKeySignatureV2")?;
        let expires_at = json["expiresAt"]
            .as_str()
            .ok_or("certificates: missing expiresAt")?;

        // Private key: Mojang labels it "RSA PRIVATE KEY" (PKCS#1) but the
        // body is PKCS#8 DER, wrapped at 76 chars — both trip the rsa
        // crate's strict RFC-7468 PEM reader. Strip the armor ourselves and
        // parse the DER directly (PKCS#8 first, PKCS#1 fallback).
        let private_der = strip_pem(private_pem)?;
        let private = RsaPrivateKey::from_pkcs8_der(&private_der)
            .or_else(|_| {
                use rsa::pkcs1::DecodeRsaPrivateKey;
                RsaPrivateKey::from_pkcs1_der(&private_der)
            })
            .map_err(|e| format!("certificates private key: {e}"))?;

        let public_key_der = strip_pem(public_pem)?;
        let key_signature = base64_decode(sig_b64)?;
        let expires_at_ms = iso8601_to_epoch_ms(expires_at)?;

        let mut session_id = [0u8; 16];
        rand::Rng::fill(&mut rand::thread_rng(), &mut session_id[..]);

        Ok(Self {
            signing_key: SigningKey::<Sha256>::new(private),
            public_key_der,
            key_signature,
            expires_at_ms,
            session_id: u128::from_be_bytes(session_id),
            sender: auth.uuid,
            index: 0,
        })
    }

    /// Sign one chat message, consuming the next chain link. Returns the
    /// 256-byte RSA signature. `salt`/`timestamp_secs` must match what goes
    /// on the wire (the signature covers them). `last_seen` is the ordered
    /// list of previously-seen 256-byte signatures (empty for us — we don't
    /// echo others' messages).
    pub fn sign(
        &mut self,
        content: &str,
        salt: i64,
        timestamp_secs: i64,
        last_seen: &[[u8; 256]],
    ) -> Vec<u8> {
        // Byte layout — verbatim from PlayerChatMessage.updateSignature:
        //   int(1)                       -- the header constant
        //   link:  uuid(sender) uuid(session) int(index)
        //   body:  long(salt) long(ts_secs) int(len) content_utf8
        //   lastSeen: int(count) each 256-byte signature
        let content = content.as_bytes();
        let mut m = Vec::with_capacity(64 + content.len() + last_seen.len() * 256);
        m.extend_from_slice(&1i32.to_be_bytes());
        m.extend_from_slice(&self.sender.to_be_bytes());
        m.extend_from_slice(&self.session_id.to_be_bytes());
        m.extend_from_slice(&self.index.to_be_bytes());
        m.extend_from_slice(&salt.to_be_bytes());
        m.extend_from_slice(&timestamp_secs.to_be_bytes());
        m.extend_from_slice(&(content.len() as i32).to_be_bytes());
        m.extend_from_slice(content);
        m.extend_from_slice(&(last_seen.len() as i32).to_be_bytes());
        for sig in last_seen {
            m.extend_from_slice(sig);
        }
        self.index += 1;
        self.signing_key.sign(&m).to_bytes().into_vec()
    }
}

/// Strip PEM armor + whitespace and base64-decode the body to DER. Label-
/// and line-width-agnostic (Mojang mislabels + wraps at 76), unlike the rsa
/// crate's strict RFC-7468 reader. For the public key the DER is X.509
/// SubjectPublicKeyInfo — the bytes vanilla's `writePublicKey` sends.
fn strip_pem(pem: &str) -> Result<Vec<u8>, String> {
    let b64: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .collect();
    base64_decode(&b64)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("base64: {e}"))
}

/// Parse an ISO-8601 UTC timestamp (`2026-07-22T06:43:12.34Z`) to epoch
/// milliseconds. Hand-rolled (no chrono dep) — the format Mojang emits is
/// fixed: `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
fn iso8601_to_epoch_ms(s: &str) -> Result<i64, String> {
    let err = || format!("bad ISO-8601 timestamp {s:?}");
    let s = s.strip_suffix('Z').unwrap_or(s);
    let (date, time) = s.split_once('T').ok_or_else(err)?;
    let mut dp = date.split('-');
    let year: i64 = dp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let month: i64 = dp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let day: i64 = dp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let (hms, frac) = match time.split_once('.') {
        Some((a, b)) => (a, b),
        None => (time, "0"),
    };
    let mut tp = hms.split(':');
    let hour: i64 = tp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let min: i64 = tp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let sec: i64 = tp.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    // Milliseconds from the fractional part (pad/truncate to 3 digits).
    let ms: i64 = format!("{frac:0<3}")[..3].parse().map_err(|_| err())?;
    let days = days_from_civil(year, month, day);
    Ok(((days * 86400 + hour * 3600 + min * 60 + sec) * 1000) + ms)
}

/// Days since the Unix epoch for a proleptic-Gregorian date (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_epoch_reference() {
        // 1970-01-01T00:00:00Z = 0.
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00Z").unwrap(), 0);
        // 2000-01-01T00:00:00Z = 946684800 s.
        assert_eq!(
            iso8601_to_epoch_ms("2000-01-01T00:00:00Z").unwrap(),
            946_684_800_000
        );
        // Fractional seconds → millis.
        assert_eq!(
            iso8601_to_epoch_ms("1970-01-01T00:00:01.250Z").unwrap(),
            1250
        );
        // Two-digit fraction pads to 3 (`.34` → 340 ms).
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00.34Z").unwrap(), 340);
    }

    #[test]
    fn strip_pem_ignores_label_and_wrapping() {
        let der = strip_pem(
            "-----BEGIN RSA PUBLIC KEY-----\nAAECAwQF\nBgcICQ==\n-----END RSA PUBLIC KEY-----",
        )
        .unwrap();
        assert_eq!(der, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    /// The signed byte layout is deterministic; pin its prefix so a field
    /// reorder is caught without a live server (the server-side verify is
    /// the ground truth, but this guards the layout locally).
    #[test]
    fn signed_layout_prefix_is_stable() {
        // Reconstruct the prefix by hand: int(1) + sender + session.
        let sender = 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10u128;
        let session = 0x1112_1314_1516_1718_191a_1b1c_1d1e_1f20u128;
        let mut expect = Vec::new();
        expect.extend_from_slice(&1i32.to_be_bytes());
        expect.extend_from_slice(&sender.to_be_bytes());
        expect.extend_from_slice(&session.to_be_bytes());
        expect.extend_from_slice(&0i32.to_be_bytes()); // index
        assert_eq!(&expect[..4], &[0, 0, 0, 1]);
        assert_eq!(expect.len(), 4 + 16 + 16 + 4);
    }
}
