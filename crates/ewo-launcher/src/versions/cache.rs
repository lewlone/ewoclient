//! On-disk cache of the version manifest at
//! `<config>/EwoClient/versions_cache.json`.
//!
//! Cached for 6 hours. The cache file stores both the manifest body and
//! the timestamp it was fetched. On launch we hydrate from disk
//! immediately (so the dropdown isn't empty during startup), then
//! background-refresh if the cache is older than `MAX_AGE`.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::manifest::VersionManifest;

const FILENAME: &str = "versions_cache.json";
/// Refresh stale caches in the background after this elapses. Mojang
/// updates the manifest a few times a week (snapshot releases), so 6h is
/// a comfortable balance between freshness and offline-friendliness.
pub const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    /// Schema version. Bump on incompatible format changes.
    #[serde(default = "default_version")]
    version: u32,
    /// Unix-epoch seconds when the manifest was fetched. Used by
    /// `is_stale()` to decide if a refresh is due.
    fetched_at: u64,
    manifest: VersionManifest,
}

fn default_version() -> u32 {
    1
}

fn cache_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(FILENAME);
    Some(p)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hydrate the cached manifest, if any. Returns `None` for any failure
/// mode (missing, malformed, version mismatch) — callers fall back to
/// fetching fresh.
pub fn load() -> Option<(VersionManifest, u64)> {
    let path = cache_path()?;
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<CacheFile>(&s) {
            Ok(c) if c.version == 1 => {
                log::info!(
                    "versions: hydrated {} entries from cache (fetched_at={})",
                    c.manifest.versions.len(),
                    c.fetched_at,
                );
                Some((c.manifest, c.fetched_at))
            }
            Ok(c) => {
                log::warn!("versions: cache has unknown version {} — ignoring", c.version);
                None
            }
            Err(e) => {
                log::warn!("versions: cache parse failed: {} — refetching", e);
                None
            }
        },
        Err(e) => {
            log::warn!("versions: cache read failed: {}", e);
            None
        }
    }
}

/// Persist a freshly-fetched manifest. Best-effort — failures log a
/// warning but don't surface (worst case, we re-fetch next launch).
pub fn save(manifest: &VersionManifest) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("versions: could not create {}: {}", parent.display(), e);
            return;
        }
    }
    let file = CacheFile {
        version: 1,
        fetched_at: now_unix(),
        manifest: manifest.clone(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                log::warn!("versions: cache write failed: {}", e);
            } else {
                log::info!(
                    "versions: cached {} entries to {}",
                    manifest.versions.len(),
                    path.display(),
                );
            }
        }
        Err(e) => log::warn!("versions: cache serialize failed: {}", e),
    }
}

/// `true` if the cached `fetched_at` is older than `MAX_AGE`.
pub fn is_stale(fetched_at: u64) -> bool {
    let now = now_unix();
    now.saturating_sub(fetched_at) > MAX_AGE.as_secs()
}
