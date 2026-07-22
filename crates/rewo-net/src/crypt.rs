//! M7 online-mode crypto — the vanilla login-encryption handshake.
//!
//! Protocol (decompiled `LoginProtocol` / `ServerboundKeyPacket`): the
//! server's clientbound `hello` carries an RSA public key (X.509
//! SubjectPublicKeyInfo DER) + a verify token. The client generates a
//! 16-byte shared secret, (if `should_authenticate`) POSTs the Mojang
//! session-join with the server hash, RSA-PKCS1v15-encrypts secret +
//! token into the serverbound `key`, and from the next byte on the whole
//! TCP stream is AES-128-CFB8 in both directions (key = IV = the secret).

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;

/// The launcher's account handoff (`REWO_ACCESS_TOKEN` / `REWO_UUID` /
/// `REWO_USERNAME` env contract) — everything the session join needs.
pub struct OnlineAuth {
    /// Minecraft services bearer token.
    pub access_token: String,
    /// Profile UUID.
    pub uuid: u128,
    /// Profile name (goes into the login `hello`).
    pub username: String,
}

impl OnlineAuth {
    /// Read the launcher env contract. `None` when unset (offline mode).
    pub fn from_env() -> Option<Self> {
        let access_token = std::env::var("REWO_ACCESS_TOKEN").ok()?;
        let uuid_s = std::env::var("REWO_UUID").ok()?;
        let username = std::env::var("REWO_USERNAME").ok()?;
        if access_token.is_empty() || uuid_s.is_empty() {
            return None;
        }
        let uuid = parse_uuid(&uuid_s)?;
        Some(Self { access_token, uuid, username })
    }
}

/// Parse a UUID with or without dashes into its u128.
pub fn parse_uuid(s: &str) -> Option<u128> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

// ---------------------------------------------------------------------------
// AES-128-CFB8
// ---------------------------------------------------------------------------

/// One direction of AES-128-CFB8. CFB *de*cryption also uses block
/// **en**cryption — the directions differ only in which byte (plaintext vs
/// ciphertext) feeds the IV shift register.
pub struct Cfb8 {
    cipher: Aes128,
    iv: [u8; 16],
}

impl Cfb8 {
    pub fn new(secret: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(secret.into()),
            iv: *secret,
        }
    }

    pub fn encrypt(&mut self, buf: &mut [u8]) {
        for b in buf {
            let c = self.keystream_byte() ^ *b;
            self.shift(c);
            *b = c;
        }
    }

    pub fn decrypt(&mut self, buf: &mut [u8]) {
        for b in buf {
            let c = *b;
            *b = self.keystream_byte() ^ c;
            self.shift(c);
        }
    }

    fn keystream_byte(&self) -> u8 {
        let mut block = self.iv.into();
        self.cipher.encrypt_block(&mut block);
        block[0]
    }

    fn shift(&mut self, feedback: u8) {
        self.iv.copy_within(1.., 0);
        self.iv[15] = feedback;
    }
}

// ---------------------------------------------------------------------------
// Mojang server hash + session join
// ---------------------------------------------------------------------------

/// Mojang's login digest: SHA-1 over `server_id ++ secret ++ pubkey_der`,
/// rendered as a *signed two's-complement* hex integer (leading zeros
/// stripped, `-` prefix when the sign bit is set) — Java `BigInteger`
/// semantics, the classic Minecraft quirk.
pub fn server_hash(server_id: &str, secret: &[u8], pubkey_der: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(server_id.as_bytes());
    h.update(secret);
    h.update(pubkey_der);
    let mut d: [u8; 20] = h.finalize().into();
    let neg = d[0] & 0x80 != 0;
    if neg {
        // Two's-complement negate the 160-bit value.
        let mut carry = true;
        for b in d.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (nb, c) = b.overflowing_add(1);
                *b = nb;
                carry = c;
            }
        }
    }
    let mut hex = String::with_capacity(41);
    if neg {
        hex.push('-');
    }
    let mut significant = false;
    for b in d {
        for nibble in [b >> 4, b & 0xf] {
            if nibble != 0 {
                significant = true;
            }
            if significant {
                hex.push(char::from_digit(nibble as u32, 16).unwrap());
            }
        }
    }
    if !significant {
        hex.push('0');
    }
    hex
}

/// `POST sessionserver.mojang.com/session/minecraft/join` — proves to
/// Mojang that this token intends to join `hash`'s server. 204 = success.
pub fn session_join(auth: &OnlineAuth, hash: &str) -> Result<(), String> {
    // All three values are base64url / hex alphabets — no JSON escaping
    // needed, so a format! body keeps serde out of this crate.
    let body = format!(
        r#"{{"accessToken":"{}","selectedProfile":"{:032x}","serverId":"{}"}}"#,
        auth.access_token, auth.uuid, hash
    );
    let resp = ureq::post("https://sessionserver.mojang.com/session/minecraft/join")
        .set("Content-Type", "application/json")
        .send_string(&body);
    match resp {
        Ok(r) if r.status() == 204 => Ok(()),
        Ok(r) => Err(format!("session join: unexpected status {}", r.status())),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            Err(format!("session join: http {code} {msg}"))
        }
        Err(e) => Err(format!("session join: {e}")),
    }
}

/// RSA-PKCS1v15 encrypt with the server's X.509-DER public key (the shared
/// secret + verify token going into the `key` packet).
pub fn rsa_encrypt(pubkey_der: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    use rsa::pkcs8::DecodePublicKey;
    let key = rsa::RsaPublicKey::from_public_key_der(pubkey_der)
        .map_err(|e| format!("server public key: {e}"))?;
    key.encrypt(&mut rand::thread_rng(), rsa::Pkcs1v15Encrypt, data)
        .map_err(|e| format!("rsa encrypt: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// NIST SP 800-38A F.3.7 (CFB8-AES128.Encrypt) known-answer test.
    #[test]
    fn cfb8_nist_kat() {
        let key: [u8; 16] = unhex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let mut c = Cfb8 {
            cipher: Aes128::new((&key).into()),
            iv,
        };
        let mut buf = unhex("6bc1bee22e409f96e93d7e117393172aae2d");
        c.encrypt(&mut buf);
        assert_eq!(buf, unhex("3b79424c9c0dd436bace9e0ed4586a4f32b9"));
        // Decrypt round-trips with a fresh state.
        let mut d = Cfb8 {
            cipher: Aes128::new((&key).into()),
            iv,
        };
        d.decrypt(&mut buf);
        assert_eq!(buf, unhex("6bc1bee22e409f96e93d7e117393172aae2d"));
    }

    /// Byte-at-a-time chunking must be transparent (the stream cipher is
    /// applied per read/write of arbitrary sizes).
    #[test]
    fn cfb8_chunking_transparent() {
        let secret = [7u8; 16];
        let msg: Vec<u8> = (0u8..100).collect();
        let mut whole = msg.clone();
        Cfb8::new(&secret).encrypt(&mut whole);
        let mut parts = msg.clone();
        let mut c = Cfb8::new(&secret);
        for chunk in parts.chunks_mut(7) {
            c.encrypt(chunk);
        }
        assert_eq!(whole, parts);
    }

    /// wiki.vg's canonical server-hash vectors (name as the sole input).
    #[test]
    fn server_hash_vectors() {
        assert_eq!(server_hash("Notch", &[], &[]), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(server_hash("jeb_", &[], &[]), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(server_hash("simon", &[], &[]), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn uuid_parses_both_forms() {
        let dashed = parse_uuid("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap();
        let plain = parse_uuid("069a79f444e94726a5befca90e38aaf5").unwrap();
        assert_eq!(dashed, plain);
        assert_eq!(format!("{plain:032x}"), "069a79f444e94726a5befca90e38aaf5");
        assert!(parse_uuid("nope").is_none());
    }
}
