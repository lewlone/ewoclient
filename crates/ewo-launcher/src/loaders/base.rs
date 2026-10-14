//! Where the EwoLoader manifests come from, resolved at *launch* time.
//!
//! Instances used to persist (and the launcher re-keyed on every start) a
//! hardcoded `file:///C:/Users/<dev>/...` manifest URL, so on any other
//! machine, and in the movable `package.ps1` bundle, the fetch failed and
//! the instance silently launched vanilla. Now the instance stores only a
//! neutral marker (`ewo-loader:<line>`) and the base is resolved here, from
//! the machine the launcher is actually running on:
//!
//! 1. `EWO_LOADER_BASE` env var (a `file://` or `https://` base URL);
//! 2. `<exe_dir>/loader/0.1.0/` when that directory exists (the bundle that
//!    `package.ps1` stages);
//! 3. debug builds only: the developer's sibling EwoLoaderV1 checkout;
//! 4. otherwise none, which is a hard launch error, never a vanilla fallback.

use std::path::{Path, PathBuf};

/// Env var that overrides the loader base.
pub const BASE_ENV: &str = "EWO_LOADER_BASE";

/// Bundled manifests live at `<exe_dir>/loader/0.1.0/<line>.json`.
pub const BUNDLED_SUBDIR: &str = "loader/0.1.0";

/// Prefix of the neutral value stored in `InstanceLoader::Ewo.manifest_url`.
/// Older `instances.toml` files hold a full (machine-specific) URL there
/// instead; the stored value is never used to fetch, only the version is.
pub const STORED_PREFIX: &str = "ewo-loader:";

/// The developer checkout of the sibling EwoLoader repo. Debug builds only:
/// a release binary must not carry a machine-specific path.
#[cfg(debug_assertions)]
const DEV_BASE: Option<&str> = Some("file:///C:/Users/valtteri/Desktop/EwoLoaderV1/manifest/0.1.0");
#[cfg(not(debug_assertions))]
const DEV_BASE: Option<&str> = None;

/// Manifests are keyed by version *line* (major.minor), so patch releases map
/// onto their line's manifest: "26.2.1" gives "26.2", "26.1" stays "26.1".
pub fn version_line(version_id: &str) -> &str {
    match version_id.match_indices('.').nth(1) {
        Some((second_dot, _)) => &version_id[..second_dot],
        None => version_id,
    }
}

/// The value new Ewo instances persist. Carries the line for readability
/// only; no base, so it is valid on every machine.
pub fn stored_marker(version_id: &str) -> String {
    format!("{}{}", STORED_PREFIX, version_line(version_id))
}

/// Pure base selection (see the module doc for the order). `env` is the raw
/// `EWO_LOADER_BASE` value, `bundled_dir` the bundle directory if it exists,
/// `dev` the debug-only developer base. The result has no trailing `/`.
pub fn pick_base(env: Option<&str>, bundled_dir: Option<&Path>, dev: Option<&str>) -> Option<String> {
    let clean = |s: &str| s.trim().trim_end_matches('/').to_string();
    if let Some(e) = env.map(clean).filter(|s| !s.is_empty()) {
        return Some(e);
    }
    if let Some(dir) = bundled_dir {
        return Some(crate::util::path_to_file_url(dir).trim_end_matches('/').to_string());
    }
    dev.map(clean).filter(|s| !s.is_empty())
}

fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join(BUNDLED_SUBDIR);
    dir.is_dir().then_some(dir)
}

/// The base for this process: env, bundle next to the exe, debug dev path.
pub fn resolve_base() -> Option<String> {
    pick_base(
        std::env::var(BASE_ENV).ok().as_deref(),
        bundled_dir().as_deref(),
        DEV_BASE,
    )
}

/// Pure: the manifest URL for `version_id` under `base`, or a user-facing
/// error when there is no base.
pub fn manifest_url_for(version_id: &str, base: Option<&str>) -> Result<String, String> {
    let base = base.ok_or_else(|| {
        format!(
            "no EwoLoader manifests found: expected a `{}` folder next to the launcher, \
             or set {} to a manifest base URL",
            BUNDLED_SUBDIR, BASE_ENV
        )
    })?;
    Ok(format!("{}/{}.json", base.trim_end_matches('/'), version_line(version_id)))
}

/// The manifest URL to fetch for an Ewo instance on `version_id`, resolved
/// now. Whatever the instance stored (the marker, or a legacy full URL from
/// an older launcher) is deliberately ignored.
pub fn resolve_manifest_url(version_id: &str) -> Result<String, String> {
    manifest_url_for(version_id, resolve_base().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_wins_and_is_cleaned() {
        let bundle = Path::new("/opt/ewo/loader/0.1.0");
        assert_eq!(
            pick_base(Some(" https://meta.example/m/ "), Some(bundle), Some("file:///dev")),
            Some("https://meta.example/m".to_string())
        );
    }

    #[test]
    fn empty_env_falls_through_to_the_bundle() {
        let bundle = Path::new("/opt/ewo/loader/0.1.0");
        let got = pick_base(Some("  "), Some(bundle), Some("file:///dev")).unwrap();
        assert!(got.starts_with("file:///"), "{got}");
        assert!(got.ends_with("opt/ewo/loader/0.1.0"), "{got}");
    }

    #[test]
    fn bundle_beats_the_dev_path_and_dev_is_the_last_resort() {
        let bundle = Path::new("/opt/ewo/loader/0.1.0");
        let got = pick_base(None, Some(bundle), Some("file:///dev/")).unwrap();
        assert!(got.ends_with("loader/0.1.0"), "{got}");
        assert_eq!(pick_base(None, None, Some("file:///dev/")), Some("file:///dev".to_string()));
    }

    #[test]
    fn no_inputs_means_no_base() {
        assert_eq!(pick_base(None, None, None), None);
        assert_eq!(pick_base(Some(""), None, None), None);
    }

    #[test]
    fn manifest_url_is_keyed_by_version_line() {
        let b = Some("file:///b/");
        assert_eq!(manifest_url_for("26.2.1", b).unwrap(), "file:///b/26.2.json");
        assert_eq!(manifest_url_for("26.1", b).unwrap(), "file:///b/26.1.json");
        assert_eq!(manifest_url_for("1.21.4", b).unwrap(), "file:///b/1.21.json");
    }

    #[test]
    fn missing_base_is_an_error_naming_the_fix() {
        let e = manifest_url_for("26.2", None).unwrap_err();
        assert!(e.contains(BASE_ENV) && e.contains("loader/0.1.0"), "{e}");
    }

    #[test]
    fn stored_marker_has_no_machine_path() {
        assert_eq!(stored_marker("26.2.1"), "ewo-loader:26.2");
    }

    /// Release builds must not carry the developer path (the const is the
    /// only place it appears and is `cfg(debug_assertions)`-gated).
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_has_no_dev_base() {
        assert!(DEV_BASE.is_none());
    }
}
