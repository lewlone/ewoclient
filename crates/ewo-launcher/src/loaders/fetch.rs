//! Fetch a `LoaderManifest` from an HTTP URL or `file://` path.
//!
//! Loader manifests are tiny (a few KB) and expected to change frequently
//! during loader development, so we deliberately skip the per-version
//! sha1-pinning + permanent-cache strategy used by `versions::per_version_fetch`.
//! Instead we re-fetch on every call but write a copy to
//! `<config>/EwoClient/shared/loaders/<id>.json` for offline / debug use.
//!
//! `file://` support is required so the user's in-development EwoLoader
//! manifest can be consumed straight off disk without standing up an
//! HTTP endpoint. The URL scheme is the only gate — local-path safety is
//! the caller's responsibility.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::manifest::LoaderManifest;

#[derive(Debug, Clone)]
pub enum FetchError {
    /// HTTP failed at fetch time, or `file://` path couldn't be read.
    Network(String),
    /// Body didn't parse as `LoaderManifest` JSON.
    Parse(String),
    /// Disk I/O failed (couldn't write cache, etc.).
    Disk(String),
    /// Malformed URL or unsupported scheme.
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(s) => write!(f, "network: {}", s),
            FetchError::Parse(s) => write!(f, "parse: {}", s),
            FetchError::Disk(s) => write!(f, "disk: {}", s),
            FetchError::Other(s) => write!(f, "other: {}", s),
        }
    }
}

impl std::error::Error for FetchError {}

/// Fetch a loader manifest by URL. `id` is the loader's logical name and
/// keys the on-disk copy at `<config>/EwoClient/shared/loaders/<id>.json`.
///
/// HTTP and `file://` schemes are supported. Other schemes return
/// `FetchError::Other`.
///
/// The fetch happens every call — there is no TTL cache. The on-disk copy
/// is written best-effort (failure is logged, not surfaced) and exists
/// purely for offline inspection. If the loader is ever shipped with a
/// stable URL + sha pinning, this is the spot to add a real cache.
pub fn get_or_fetch(id: &str, url: &str) -> Result<LoaderManifest, FetchError> {
    let body = if let Some(path) = crate::util::file_url_to_path(url) {
        log::info!("loader: reading {} from file {}", id, path.display());
        fs::read_to_string(&path)
            .map_err(|e| FetchError::Network(format!("read {}: {}", path.display(), e)))?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        log::info!("loader: fetching {} from {}", id, url);
        fetch_http(url)?
    } else {
        return Err(FetchError::Other(format!("unsupported URL scheme: {}", url)));
    };

    let parsed: LoaderManifest = serde_json::from_str(&body)
        .map_err(|e| FetchError::Parse(e.to_string()))?;

    if let Err(e) = save_cached(id, &body) {
        log::warn!("loader: cache write failed: {}", e);
    }

    Ok(parsed)
}

fn fetch_http(url: &str) -> Result<String, FetchError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    match agent.get(url).call() {
        Ok(r) => r
            .into_string()
            .map_err(|e| FetchError::Network(format!("read body: {}", e))),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(FetchError::Network(format!("{} HTTP {}", code, body)))
        }
        Err(e) => Err(FetchError::Network(e.to_string())),
    }
}

fn loaders_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared");
    p.push("loaders");
    Some(p)
}

pub fn cached_path(id: &str) -> Option<PathBuf> {
    let mut p = loaders_dir()?;
    p.push(format!("{}.json", id));
    Some(p)
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
    log::info!("loader: cached {} to {}", id, path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::Write;

    #[test]
    fn file_url_round_trips_through_get_or_fetch() {
        // Synthesize a minimal loader manifest, write it to a temp file,
        // then ensure get_or_fetch can read it back via `file://`.
        let mut path = env::temp_dir();
        path.push(format!(
            "ewoclient_test_loader_{}.json",
            std::process::id()
        ));
        let manifest_json = r#"{
            "id": "ewo-test-0.0.1",
            "inheritsFrom": "1.21.4",
            "mainClass": "net.example.Main",
            "libraries": [],
            "releaseTime": "2026-05-10T00:00:00+00:00"
        }"#;
        {
            let mut f = std::fs::File::create(&path).expect("create temp");
            f.write_all(manifest_json.as_bytes()).expect("write temp");
        }

        // Build a `file://` URL. On Windows the path is `C:\...`; convert
        // backslashes + prepend an extra `/` to match the
        // `file:///C:/...` shape.
        let url = if cfg!(windows) {
            format!(
                "file:///{}",
                path.to_string_lossy().replace('\\', "/")
            )
        } else {
            format!("file://{}", path.to_string_lossy())
        };

        let result = get_or_fetch("ewo-test-0.0.1", &url);
        let _ = std::fs::remove_file(&path);
        let parsed = result.expect("fetch should succeed");
        assert_eq!(parsed.id, "ewo-test-0.0.1");
        assert_eq!(parsed.inherits_from, "1.21.4");
        assert_eq!(parsed.main_class.as_deref(), Some("net.example.Main"));
    }

    #[test]
    fn unsupported_scheme_errors() {
        let err = get_or_fetch("nope", "ftp://example.com/x.json").unwrap_err();
        match err {
            FetchError::Other(_) => {}
            other => panic!("expected Other, got {:?}", other),
        }
    }
}
