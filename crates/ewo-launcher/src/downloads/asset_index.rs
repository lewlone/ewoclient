//! Mojang's asset index file format + the URL pattern used to fetch
//! individual asset blobs.
//!
//! The asset index lives at `<assets>/indexes/<id>.json` (id from the
//! per-version manifest's `assetIndex.id`). Inside it, `objects` maps
//! human-readable names to `{ hash, size }` pairs. Each blob is fetched
//! from
//!     `https://resources.download.minecraft.net/<2-char prefix>/<hash>`
//! and stored at
//!     `<assets>/objects/<2-char prefix>/<hash>`
//!
//! Some old indexes (≤1.6 — `pre-1.6`) use `map_to_resources: true`,
//! meaning blobs need to be copied to per-instance `resources/` paths.
//! Our oldest target is 1.8.9, which doesn't use that flag, so we
//! ignore it for now.

use std::collections::HashMap;

use serde::Deserialize;

/// CDN base URL for asset blobs.
pub const ASSETS_CDN_BASE: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    /// Some old indexes set this to copy assets into per-instance dirs.
    /// Modern (≥1.7.x) indexes omit it.
    #[serde(default)]
    pub map_to_resources: bool,
    /// `virtual: true` means the same as `map_to_resources` (old field
    /// name pre-1.7.3). We don't bother with this either.
    #[serde(default, rename = "virtual")]
    pub virtual_: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

/// Build the CDN URL for an asset blob given its sha1 hash.
pub fn asset_url(hash: &str) -> Option<String> {
    if hash.len() < 2 {
        return None;
    }
    Some(format!("{}/{}/{}", ASSETS_CDN_BASE, &hash[..2], hash))
}
