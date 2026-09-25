//! Disk path helpers for the launcher's shared / per-instance trees.
//!
//! Layout matches Mojang's official launcher conventions so existing
//! installations are interoperable + the official launcher (or any
//! well-behaved third-party launcher) can use the same files:
//!
//! ```text
//! <config>/EwoClient/
//!   shared/
//!     versions/<id>/
//!       <id>.json    ← per-version manifest
//!       <id>.jar     ← client jar
//!     libraries/
//!       <maven path>/<artifact>.jar
//!     assets/
//!       indexes/<asset_index_id>.json
//!       objects/<2-char prefix>/<sha1>
//!   instances/<id>/
//!     saves/, screenshots/, mods/, etc.
//!     natives/                ← extracted at launch time (Phase C)
//! ```

use std::path::PathBuf;

/// Root of the launcher's disk tree.
/// `<config>/EwoClient` (e.g. `%APPDATA%/EwoClient` on Windows).
pub fn root() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    Some(p)
}

/// Root of files shared across instances:
/// `<root>/shared`.
pub fn shared() -> Option<PathBuf> {
    let mut p = root()?;
    p.push("shared");
    Some(p)
}

/// Per-version directory: `<shared>/versions/<id>`.
pub fn version_dir(id: &str) -> Option<PathBuf> {
    if !is_plain_segment(id) {
        return None;
    }
    let mut p = shared()?;
    p.push("versions");
    p.push(id);
    Some(p)
}

/// Path to the client JAR for a version: `<version_dir>/<id>.jar`.
pub fn client_jar(id: &str) -> Option<PathBuf> {
    let mut p = version_dir(id)?;
    p.push(format!("{}.jar", id));
    Some(p)
}

/// Libraries dir: `<shared>/libraries`. Each library's path under here
/// comes from its `Artifact.path` field (already in Maven layout).
pub fn libraries_dir() -> Option<PathBuf> {
    let mut p = shared()?;
    p.push("libraries");
    Some(p)
}

/// Resolve a library artifact's full disk path. `artifact_path` comes from
/// a (possibly remote) manifest, so anything that could leave the libraries
/// tree — `..`, empty or absolute segments, drive prefixes, backslashes — is
/// rejected with `None`.
pub fn library_path(artifact_path: &str) -> Option<PathBuf> {
    if artifact_path.is_empty() {
        return None;
    }
    let mut p = libraries_dir()?;
    for segment in artifact_path.split('/') {
        if !is_plain_segment(segment) {
            return None;
        }
        p.push(segment);
    }
    Some(p)
}

/// A single path component that can't escape its parent directory.
fn is_plain_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains(['\\', ':', '\0'])
}

/// Assets dir: `<shared>/assets`.
pub fn assets_dir() -> Option<PathBuf> {
    let mut p = shared()?;
    p.push("assets");
    Some(p)
}

/// Asset index path: `<assets>/indexes/<id>.json`.
pub fn asset_index_path(id: &str) -> Option<PathBuf> {
    if !is_plain_segment(id) {
        return None;
    }
    let mut p = assets_dir()?;
    p.push("indexes");
    p.push(format!("{}.json", id));
    Some(p)
}

/// Asset object path: `<assets>/objects/<2-char>/<hash>`. Prefix is the
/// first 2 chars of the sha1, matching Mojang's CDN URL layout
/// (`resources.download.minecraft.net/<prefix>/<hash>`).
pub fn asset_object_path(hash: &str) -> Option<PathBuf> {
    if !is_sha1_hex(hash) {
        return None;
    }
    let prefix = &hash[..2];
    let mut p = assets_dir()?;
    p.push("objects");
    p.push(prefix);
    p.push(hash);
    Some(p)
}

/// A 40-char lowercase-or-uppercase hex sha1 — the only shape an asset
/// object key may take (it becomes two path components).
pub fn is_sha1_hex(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Root of all instance dirs: `<root>/instances`.
pub fn instances_root() -> Option<PathBuf> {
    let mut p = root()?;
    p.push("instances");
    Some(p)
}

/// Per-instance dir: `<root>/instances/<id>`, keyed by the instance's
/// stable `id` (never its display name). Worlds, screenshots, per-instance
/// config live here. `None` if `id` isn't a safe single path component.
pub fn instance_dir(id: &str) -> Option<PathBuf> {
    if crate::util::validate_name(id, 64).ok()? != id {
        return None;
    }
    let mut p = instances_root()?;
    p.push(id);
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_path_rejects_traversal() {
        assert!(library_path("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar").is_some());
        for bad in [
            "",
            "../evil.jar",
            "org/../../evil.jar",
            "/abs/evil.jar",
            "org//x.jar",
            "C:/evil.jar",
            "org\\..\\evil.jar",
        ] {
            assert!(library_path(bad).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn asset_object_path_requires_a_sha1() {
        let good = "a".repeat(40);
        assert!(asset_object_path(&good).is_some());
        assert!(asset_object_path("ab").is_none());
        assert!(asset_object_path(&format!("../{}", "a".repeat(37))).is_none());
    }

    #[test]
    fn instance_dir_rejects_unsafe_ids() {
        assert!(instance_dir("velvet-hours-1a2b3c").is_some());
        for bad in ["", "..", "a/b", "a\\b", "C:", " padded"] {
            assert!(instance_dir(bad).is_none(), "accepted {bad:?}");
        }
    }
}
