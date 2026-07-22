//! Player skin resolution — the profile `textures` property.
//!
//! The Player Info packet's ADD_PLAYER action carries GameProfile
//! properties; the `textures` one is a base64 JSON blob with the skin URL
//! and (optionally) a `metadata.model = "slim"` marker. Online-mode
//! servers relay it for every player, so once we join a real server we can
//! render each player's actual skin instead of the default Steve.

use base64::Engine;

/// A player's resolved skin: where to fetch the PNG + the arm model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinInfo {
    pub url: String,
    /// `true` = 3-px "slim" (Alex) arms; `false` = 4-px "classic" (Steve).
    pub slim: bool,
}

/// Decode a base64 `textures` property value into its skin URL + model.
/// Returns `None` if the blob has no SKIN entry (some profiles carry only
/// a cape, or none at all). Hand-parsed off `serde_json::Value` — no typed
/// schema, since the blob is small and its shape is fixed.
pub fn decode_textures_property(value_b64: &str) -> Option<SkinInfo> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value_b64.trim())
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let skin = json.get("textures")?.get("SKIN")?;
    let url = skin.get("url")?.as_str()?.to_string();
    // metadata.model == "slim" → Alex arms; absent/anything else → classic.
    let slim = skin
        .get("metadata")
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        == Some("slim");
    Some(SkinInfo { url, slim })
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
            info.url,
            "http://textures.minecraft.net/texture/6f608526fa94df402e4ac167d0ccae3baa5795d7a5ffa4fe2480a0bc46111acd"
        );
        assert!(info.slim, "metadata.model=slim");
    }

    #[test]
    fn classic_when_no_metadata() {
        // A minimal classic blob (no metadata → wide arms).
        let blob = r#"{"textures":{"SKIN":{"url":"http://x/y"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let info = decode_textures_property(&b64).unwrap();
        assert_eq!(info.url, "http://x/y");
        assert!(!info.slim);
    }

    #[test]
    fn none_when_cape_only() {
        let blob = r#"{"textures":{"CAPE":{"url":"http://x/c"}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        assert!(decode_textures_property(&b64).is_none());
    }

    #[test]
    fn none_on_garbage() {
        assert!(decode_textures_property("not base64!!!").is_none());
    }
}
