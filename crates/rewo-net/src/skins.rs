//! Player skin resolution — the profile `textures` property.
//!
//! The Player Info packet's ADD_PLAYER action carries GameProfile
//! properties; the `textures` one is a base64 JSON blob with the skin URL
//! and (optionally) a `metadata.model = "slim"` marker. Online-mode
//! servers relay it for every player, so once we join a real server we can
//! render each player's actual skin instead of the default Steve.
//!
//! The same blob carries `CAPE` (M60) and `ELYTRA`. Vanilla's `PlayerSkin`
//! is a record of all three, and `CapeLayer` gates on `skin.cape() != null`
//! alone — a profile may carry a cape and no skin, which is why every field
//! here is independently optional and why the decoder succeeds on any of
//! them rather than requiring `SKIN`.

use base64::Engine;

/// A player's resolved textures: where to fetch each PNG + the arm model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkinInfo {
    /// `textures.SKIN.url`, absent on a cape-only profile.
    pub url: Option<String>,
    /// `true` = 3-px "slim" (Alex) arms; `false` = 4-px "classic" (Steve).
    pub slim: bool,
    /// `textures.CAPE.url` — vanilla `PlayerSkin.cape()`. `None` is the
    /// no-cape case the cape layer's second gate tests for.
    pub cape: Option<String>,
    /// `textures.ELYTRA.url` — vanilla `PlayerSkin.elytra()`. Decoded so the
    /// wire is fully read; Rewo renders no elytra, so nothing consumes it
    /// yet.
    pub elytra: Option<String>,
}

impl SkinInfo {
    /// Whether the blob named any texture at all — the decoder's success
    /// condition, kept as a method so the rule has one home.
    fn any(&self) -> bool {
        self.url.is_some() || self.cape.is_some() || self.elytra.is_some()
    }
}

/// The skins a session currently tracks, by profile UUID: a live-player bound,
/// not a lifetime one. An entry leaves with its player (`Player Info Remove`)
/// and with the whole level (a reconfiguration), so [`MAX_TRACKED_SKINS`] only
/// ever counts players present at once; a hub with unbounded UUID churn keeps
/// resolving skins for whoever is actually listed.
#[derive(Debug, Default)]
pub struct SkinTable {
    map: std::collections::HashMap<u128, SkinInfo>,
    /// The cap refused a new player and the log line was written; reset when
    /// room reopens, so a later episode is reported once too.
    refusal_logged: bool,
}

impl SkinTable {
    /// Record `info` for `uuid`. Returns `true` when it is new or changed, so
    /// the caller queues exactly one fetch. `queued` is the caller's count of
    /// announced-but-undrained skins, bounded by the same cap.
    pub fn note(&mut self, uuid: u128, info: &SkinInfo, queued: usize) -> bool {
        let known = self.map.contains_key(&uuid);
        if !known && self.map.len() >= MAX_TRACKED_SKINS {
            if !self.refusal_logged {
                self.refusal_logged = true;
                log::warn!(
                    "net: {MAX_TRACKED_SKINS} player skins are tracked at once; \
                     further players keep the default skin until some leave"
                );
            }
            return false;
        }
        if queued >= MAX_TRACKED_SKINS || self.map.get(&uuid) == Some(info) {
            return false;
        }
        self.map.insert(uuid, info.clone());
        true
    }

    /// A player left the list: free their slot.
    pub fn remove(&mut self, uuid: u128) {
        if self.map.remove(&uuid).is_some() && self.map.len() < MAX_TRACKED_SKINS {
            self.refusal_logged = false;
        }
    }

    /// Everything goes (the level and its tab list were dropped).
    pub fn clear(&mut self) {
        self.map.clear();
        self.refusal_logged = false;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Longest texture URL accepted. Real ones are ~100 bytes
/// (`http://textures.minecraft.net/texture/<64 hex>`); the cap only bounds
/// what a hostile server can make the client hold per player.
pub const MAX_TEXTURE_URL_LEN: usize = 512;

/// Most players whose textures a session tracks or has queued at once. A
/// proxy network's tab list is in the hundreds; this only stops a hostile
/// server from growing the maps without bound.
pub const MAX_TRACKED_SKINS: usize = 4096;

/// Vanilla's texture-domain rule, applied to a server-supplied texture URL.
///
/// Returns the URL unchanged when it may be fetched, `None` otherwise.
///
/// Transcribed from authlib 9.0.75 `com.mojang.authlib.yggdrasil.TextureUrlChecker
/// .isAllowedTextureDomain`, the authlib the 26.2 version json pins (26.1's
/// 7.0.63 has the same rule; it is not part of the Vineflower decompile of the
/// client jar, which only calls it through `SkinManager`). The vanilla rule is:
///
/// * scheme is exactly `http` or `https` (case-sensitive, via
///   `ALLOWED_SCHEMES = Set.of("http", "https")`);
/// * a host exists, and is already lower-case (`IDN.toUnicode(host)` must equal
///   its own `toLowerCase(Locale.ROOT)`);
/// * the host is exactly `textures.minecraft.net` (`ALLOWED_DOMAINS.contains`,
///   a one-element set).
///
/// Older authlib (7.0.61, 1.21.11's) allowed any `*.minecraft.net` /
/// `*.mojang.com` host minus a block list; 26.x narrowed it to the one host.
///
/// Vanilla then hands the string to `java.net.URI`, whose view of the host can
/// differ from what an HTTP client resolves (`http://textures.minecraft.net@evil/`,
/// backslashes). Rewo's HTTP client is not `java.net`, so this checker is
/// deliberately **stricter** than vanilla and rejects every input where two
/// parsers could disagree: userinfo (`@`), ports, backslashes, `%`/non-ASCII/
/// control characters or whitespace in the authority, IP literals, empty labels
/// and a trailing dot. Real texture URLs are plain `scheme://host/path`, so
/// nothing legitimate is lost.
pub fn allowed_texture_url(url: &str) -> Option<&str> {
    const ALLOWED_DOMAINS: [&str; 1] = ["textures.minecraft.net"];
    if url.len() > MAX_TEXTURE_URL_LEN
        || !url.bytes().all(|b| b.is_ascii_graphic())
        || url.contains('\\')
    {
        return None;
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    // Lower-case letters, digits, `-` and `.` only: no `@`, `:` (port / IPv6),
    // `[`, `%`, and no upper case (vanilla rejects it too).
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    {
        return None;
    }
    if host.starts_with('.') || host.contains("..") {
        return None;
    }
    // An all-numeric dotted host is an IP literal; it cannot satisfy the
    // host rule below, but say so explicitly rather than lean on it.
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    ALLOWED_DOMAINS.contains(&host).then_some(url)
}

/// Keep a decoded texture URL only if [`allowed_texture_url`] accepts it.
fn vet(kind: &str, url: String) -> Option<String> {
    if allowed_texture_url(&url).is_some() {
        Some(url)
    } else {
        // Debug only: the server picks the text, so keep it out of warn-level
        // logs, and cap what is echoed.
        let shown: String = url.chars().take(80).collect();
        log::debug!("skins: dropped disallowed {kind} texture URL {shown:?}");
        None
    }
}

/// Decode a base64 `textures` property value into its texture URLs + model.
///
/// Every URL must pass [`allowed_texture_url`]; one that does not is dropped
/// (that texture falls back to the default) and logged at debug level.
///
/// Returns `None` only when the blob is unreadable or names no texture at
/// all. Hand-parsed off `serde_json::Value` — no typed schema, since the
/// blob is small and its shape is fixed.
pub fn decode_textures_property(value_b64: &str) -> Option<SkinInfo> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value_b64.trim())
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let textures = json.get("textures")?;
    let url_of = |key: &str| {
        textures
            .get(key)
            .and_then(|t| t.get("url"))
            .and_then(|u| u.as_str())
            .and_then(|s| vet(key, s.to_string()))
    };
    // metadata.model == "slim" → Alex arms; absent/anything else → classic.
    let slim = textures
        .get("SKIN")
        .and_then(|s| s.get("metadata"))
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        == Some("slim");
    let info = SkinInfo {
        url: url_of("SKIN"),
        slim,
        cape: url_of("CAPE"),
        elytra: url_of("ELYTRA"),
    };
    info.any().then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skin(n: u128) -> SkinInfo {
        SkinInfo {
            url: Some(format!("http://textures.minecraft.net/texture/{n:064x}")),
            ..SkinInfo::default()
        }
    }

    /// A hub with churn: far more distinct UUIDs than the cap pass through, but
    /// never more than a few at once. Every one must still get a skin.
    #[test]
    fn churn_past_the_cap_still_resolves_skins() {
        let mut t = SkinTable::default();
        for n in 0..(3 * MAX_TRACKED_SKINS as u128) {
            assert!(t.note(n, &skin(n), 0), "player {n} was refused");
            t.remove(n);
        }
        assert!(t.is_empty());
    }

    #[test]
    fn the_cap_bounds_live_players_and_reopens_on_removal() {
        let mut t = SkinTable::default();
        for n in 0..MAX_TRACKED_SKINS as u128 {
            assert!(t.note(n, &skin(n), 0));
        }
        assert_eq!(t.len(), MAX_TRACKED_SKINS);
        let extra = MAX_TRACKED_SKINS as u128;
        assert!(!t.note(extra, &skin(extra), 0), "a live cap still refuses");
        assert!(t.refusal_logged);
        // A known player's change is still accepted at the cap.
        assert!(t.note(0, &skin(9999), 0));
        assert!(
            !t.note(0, &skin(9999), 0),
            "an unchanged skin is not requeued"
        );
        t.remove(1);
        assert!(!t.refusal_logged, "room reopened, so the next episode logs");
        assert!(t.note(extra, &skin(extra), 0));
    }

    #[test]
    fn the_pending_queue_is_bounded_too() {
        let mut t = SkinTable::default();
        assert!(!t.note(1, &skin(1), MAX_TRACKED_SKINS));
        assert!(t.is_empty());
    }

    #[test]
    fn clear_drops_every_entry() {
        let mut t = SkinTable::default();
        t.note(1, &skin(1), 0);
        t.clear();
        assert!(t.is_empty());
        assert!(t.note(1, &skin(1), 0), "a returning player is queued again");
    }

    /// A real `textures` value captured from a live profile (slim model).
    /// Guards the exact base64 → JSON → {url, slim} path against a wire
    /// value, not a hand-built one.
    const REAL_SLIM: &str = "ewogICJ0aW1lc3RhbXAiIDogMTc4NDcwNDAzMTk5NSwKICAicHJvZmlsZUlkIiA6ICI5Y2JiOGZiMzE1ZDM0ZjY1ODNjMGZhOTBiYmVjNGY5MiIsCiAgInByb2ZpbGVOYW1lIiA6ICJsZXdsb25lIiwKICAidGV4dHVyZXMiIDogewogICAgIlNLSU4iIDogewogICAgICAidXJsIiA6ICJodHRwOi8vdGV4dHVyZXMubWluZWNyYWZ0Lm5ldC90ZXh0dXJlLzZmNjA4NTI2ZmE5NGRmNDAyZTRhYzE2N2QwY2NhZTNiYWE1Nzk1ZDdhNWZmYTRmZTI0ODBhMGJjNDYxMTFhY2QiLAogICAgICAibWV0YWRhdGEiIDogewogICAgICAgICJtb2RlbCIgOiAic2xpbSIKICAgICAgfQogICAgfQogIH0KfQ==";

    #[test]
    fn decodes_real_slim_property() {
        let info = decode_textures_property(REAL_SLIM).expect("has a skin");
        assert_eq!(
            info.url.as_deref(),
            Some("http://textures.minecraft.net/texture/6f608526fa94df402e4ac167d0ccae3baa5795d7a5ffa4fe2480a0bc46111acd")
        );
        assert!(info.slim, "metadata.model=slim");
        assert!(info.cape.is_none(), "this profile wears no cape");
    }

    #[test]
    fn classic_when_no_metadata() {
        // A minimal classic blob (no metadata → wide arms).
        let blob = r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/y"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://textures.minecraft.net/y"));
        assert!(!info.slim);
    }

    /// M60 reversed this: the pre-cape decoder required a `SKIN` entry and
    /// returned `None` here, which made a cape-only profile invisible to the
    /// whole client. `CapeLayer`'s gate is `skin.cape() != null`, so the
    /// cape has to survive on its own.
    #[test]
    fn cape_only_profile_resolves_a_cape() {
        let blob = r#"{"textures":{"CAPE":{"url":"http://textures.minecraft.net/c"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).expect("a cape is a texture");
        assert_eq!(
            info.cape.as_deref(),
            Some("http://textures.minecraft.net/c")
        );
        assert!(info.url.is_none(), "and it carries no skin");
        assert!(!info.slim, "no SKIN metadata → the classic default");
    }

    #[test]
    fn reads_skin_and_cape_together() {
        let blob = r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/s","metadata":{"model":"slim"}},"CAPE":{"url":"http://textures.minecraft.net/c"},"ELYTRA":{"url":"http://textures.minecraft.net/e"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://textures.minecraft.net/s"));
        assert_eq!(
            info.cape.as_deref(),
            Some("http://textures.minecraft.net/c")
        );
        assert_eq!(
            info.elytra.as_deref(),
            Some("http://textures.minecraft.net/e")
        );
        assert!(info.slim);
    }

    #[test]
    fn none_when_the_blob_names_nothing() {
        let blob = r#"{"textures":{}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        assert!(decode_textures_property(&b64).is_none());
    }

    #[test]
    fn none_on_garbage() {
        assert!(decode_textures_property("not base64!!!").is_none());
    }

    const HASH: &str = "6f608526fa94df402e4ac167d0ccae3baa5795d7a5ffa4fe2480a0bc46111acd";

    #[test]
    fn allows_the_real_vanilla_urls_over_http_and_https() {
        // Mojang's session server still hands out `http://`; vanilla accepts it.
        for scheme in ["http", "https"] {
            let u = format!("{scheme}://textures.minecraft.net/texture/{HASH}");
            assert_eq!(allowed_texture_url(&u), Some(u.as_str()), "{u}");
        }
        assert!(allowed_texture_url("https://textures.minecraft.net").is_some());
        assert!(allowed_texture_url("https://textures.minecraft.net?x=1").is_some());
    }

    #[test]
    fn rejects_other_hosts_and_other_schemes() {
        for u in [
            "https://example.com/texture/x",
            "https://minecraft.net/x",
            "https://mojang.com/x",
            // 26.x allows the one host; older authlib's *.minecraft.net and
            // *.mojang.com wildcards are gone.
            "https://sub.domain.mojang.com/x",
            "https://skins.minecraft.net/x",
            "https://textures.mojang.com/x",
            "ftp://textures.minecraft.net/x",
            "file:///etc/passwd",
            "HTTP://textures.minecraft.net/x", // vanilla's scheme set is case-sensitive
            "//textures.minecraft.net/x",
            "textures.minecraft.net/x",
            "",
        ] {
            assert!(allowed_texture_url(u).is_none(), "{u}");
        }
    }

    #[test]
    fn rejects_suffix_and_userinfo_tricks() {
        for u in [
            "https://textures.minecraft.net.evil.com/x",
            "https://evil.com/textures.minecraft.net",
            "https://eviltextures.minecraft.net.evil.com/x",
            "https://textures.minecraft.net@evil.com/x",
            "https://user:pw@textures.minecraft.net/x",
            "https://evil.com#@textures.minecraft.net/x",
            "https://evil.com\\@textures.minecraft.net/x",
            "https://evil.com%2f.minecraft.net/x",
            "https://textures.minecraft.net./x", // trailing dot
            "https://.minecraft.net/x",
            "https://a..minecraft.net/x",
            "https://textures.minecraft.net:8080/x",
            "https://textures.minecraft.net:443/x",
            "https://textures.minecraft.net\n.evil.com/",
            "https://textures.minecraft.net /x",
            "https://t\u{435}xtures.minecraft.net/x", // Cyrillic e
        ] {
            assert!(allowed_texture_url(u).is_none(), "{u:?}");
        }
    }

    #[test]
    fn rejects_ip_literals_and_localhost() {
        for u in [
            "http://127.0.0.1/x",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1.minecraft.net.evil/x",
            "http://[::1]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://2130706433/x",
            "http://localhost/x",
            "http://localhost.minecraft.net.local/x",
            "http://0x7f.0.0.1/x",
        ] {
            assert!(allowed_texture_url(u).is_none(), "{u}");
        }
    }

    #[test]
    fn rejects_uppercase_hosts_like_vanilla() {
        assert!(allowed_texture_url("https://Textures.Minecraft.Net/x").is_none());
        assert!(allowed_texture_url("https://textures.MINECRAFT.net/x").is_none());
    }

    #[test]
    fn rejects_blocked_subdomains_and_overlong_urls() {
        for u in [
            "https://bugs.mojang.com/x",
            "https://education.minecraft.net/x",
            "https://feedback.minecraft.net/x",
            "https://sub.education.minecraft.net/x",
        ] {
            assert!(allowed_texture_url(u).is_none(), "{u}");
        }
        let long = format!(
            "https://textures.minecraft.net/{}",
            "a".repeat(MAX_TEXTURE_URL_LEN)
        );
        assert!(allowed_texture_url(&long).is_none());
    }

    #[test]
    fn the_decoder_drops_a_disallowed_url_and_keeps_the_rest() {
        let blob = format!(
            r#"{{"textures":{{"SKIN":{{"url":"http://169.254.169.254/s"}},"CAPE":{{"url":"https://textures.minecraft.net/texture/{HASH}"}}}}}}"#
        );
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).expect("the cape survives");
        assert!(info.url.is_none(), "hostile skin URL dropped");
        assert!(info.cape.is_some());
        // Nothing allowed at all -> no info at all.
        let blob = r#"{"textures":{"SKIN":{"url":"http://127.0.0.1/s"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        assert!(decode_textures_property(&b64).is_none());
    }
}
