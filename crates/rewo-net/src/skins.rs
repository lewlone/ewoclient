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

/// Decode a base64 `textures` property value into its texture URLs + model.
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
            .map(|s| s.to_string())
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
        let blob = r#"{"textures":{"SKIN":{"url":"http://x/y"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://x/y"));
        assert!(!info.slim);
    }

    /// M60 reversed this: the pre-cape decoder required a `SKIN` entry and
    /// returned `None` here, which made a cape-only profile invisible to the
    /// whole client. `CapeLayer`'s gate is `skin.cape() != null`, so the
    /// cape has to survive on its own.
    #[test]
    fn cape_only_profile_resolves_a_cape() {
        let blob = r#"{"textures":{"CAPE":{"url":"http://x/c"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).expect("a cape is a texture");
        assert_eq!(info.cape.as_deref(), Some("http://x/c"));
        assert!(info.url.is_none(), "and it carries no skin");
        assert!(!info.slim, "no SKIN metadata → the classic default");
    }

    #[test]
    fn reads_skin_and_cape_together() {
        let blob = r#"{"textures":{"SKIN":{"url":"http://x/s","metadata":{"model":"slim"}},"CAPE":{"url":"http://x/c"},"ELYTRA":{"url":"http://x/e"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://x/s"));
        assert_eq!(info.cape.as_deref(), Some("http://x/c"));
        assert_eq!(info.elytra.as_deref(), Some("http://x/e"));
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
}
