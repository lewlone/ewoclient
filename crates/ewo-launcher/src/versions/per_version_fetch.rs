//! Fetch + sha1-verified disk cache for per-version manifests.
//!
//! Per-version manifests are immutable once Mojang publishes them — the
//! sha1 in the master manifest's entry pins them. So caching is safe and
//! has no TTL: once fetched + verified, we keep it forever.
//!
//! Disk layout matches Mojang's official launcher convention:
//! ```text
//! <config>/EwoClient/shared/versions/<id>/<id>.json
//! ```
//! This means the official launcher (and other launchers that follow the
//! same convention) can read our cache directly, and vice versa.
//!
//! Calls are synchronous. Wrap in `std::thread::spawn` if the caller
//! needs to keep the UI responsive while a fetch is in flight.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use sha1::{Digest, Sha1};

use super::manifest::ManifestEntry;
use super::per_version::PerVersion;

#[derive(Debug, Clone)]
pub enum FetchError {
    /// The master manifest doesn't have an entry with that ID.
    UnknownVersion(String),
    /// HTTP failed at fetch time.
    Network(String),
    /// Body didn't parse as `PerVersion` JSON.
    Parse(String),
    /// SHA-1 of the fetched body didn't match the master manifest's
    /// pinned hash. This is rare but indicates either corruption in
    /// transit or a stale master manifest entry.
    Sha1Mismatch { expected: String, actual: String },
    /// Disk I/O failed (couldn't write cache, couldn't create dir, etc.).
    Disk(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::UnknownVersion(id) => write!(f, "unknown version: {}", id),
            FetchError::Network(s) => write!(f, "network: {}", s),
            FetchError::Parse(s) => write!(f, "parse: {}", s),
            FetchError::Sha1Mismatch { expected, actual } => {
                write!(f, "sha1 mismatch (expected {}, got {})", expected, actual)
            }
            FetchError::Disk(s) => write!(f, "disk: {}", s),
        }
    }
}

impl std::error::Error for FetchError {}

/// Get a per-version manifest, hitting the cache first and only fetching
/// from the network on a miss. The master `entry` is required so we can
/// (a) know the URL and (b) verify the sha1 of the fetched body.
///
/// Returns the parsed manifest. The raw JSON is cached on disk for
/// future calls; subsequent calls for the same version are zero network.
pub fn get_or_fetch(entry: &ManifestEntry) -> Result<PerVersion, FetchError> {
    if let Some(parsed) = load_cached(&entry.id) {
        log::info!("per-version: cache hit for {}", entry.id);
        return Ok(parsed);
    }
    log::info!(
        "per-version: cache miss for {} — fetching from {}",
        entry.id, entry.url
    );
    fetch_and_cache(entry)
}

fn fetch_and_cache(entry: &ManifestEntry) -> Result<PerVersion, FetchError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    let body = match agent.get(&entry.url).call() {
        Ok(r) => r
            .into_string()
            .map_err(|e| FetchError::Network(format!("read body: {}", e)))?,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            return Err(FetchError::Network(format!("{} HTTP {}", code, body)));
        }
        Err(e) => return Err(FetchError::Network(e.to_string())),
    };

    // Verify the sha1 against the master manifest's pinned hash. Mojang
    // sets `sha1` on every entry as of `version_manifest_v2.json`; if
    // it's empty (very old launcher meta data?), we skip the check.
    if !entry.sha1.is_empty() {
        let actual = sha1_hex(body.as_bytes());
        if actual != entry.sha1 {
            return Err(FetchError::Sha1Mismatch {
                expected: entry.sha1.clone(),
                actual,
            });
        }
    }

    let parsed: PerVersion = serde_json::from_str(&body)
        .map_err(|e| FetchError::Parse(e.to_string()))?;

    // Best-effort write to disk cache. If it fails we still return the
    // parsed result — the caller doesn't care that the next call will
    // re-fetch.
    if let Err(e) = save_cached(&entry.id, &body) {
        log::warn!("per-version: cache write failed: {}", e);
    }

    Ok(parsed)
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(bytes);
    let digest = h.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn versions_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared");
    p.push("versions");
    Some(p)
}

/// Path to a cached per-version manifest on disk:
/// `<config>/EwoClient/shared/versions/<id>/<id>.json`. Mirrors the
/// official launcher's layout exactly.
pub fn cached_path(id: &str) -> Option<PathBuf> {
    let mut p = versions_dir()?;
    p.push(id);
    p.push(format!("{}.json", id));
    Some(p)
}

fn load_cached(id: &str) -> Option<PerVersion> {
    let path = cached_path(id)?;
    if !path.exists() {
        return None;
    }
    let body = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&body).ok()
}

fn save_cached(id: &str, raw_body: &str) -> Result<(), FetchError> {
    let path = cached_path(id)
        .ok_or_else(|| FetchError::Disk("config dir unresolvable".into()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| FetchError::Disk(format!("mkdir {}: {}", parent.display(), e)))?;
    }
    fs::write(&path, raw_body)
        .map_err(|e| FetchError::Disk(format!("write {}: {}", path.display(), e)))?;
    log::info!("per-version: cached {} to {}", id, path.display());
    Ok(())
}
