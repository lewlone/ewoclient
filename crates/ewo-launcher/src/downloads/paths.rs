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
//!   instances/<name>/
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

/// Resolve a library artifact's full disk path.
pub fn library_path(artifact_path: &str) -> Option<PathBuf> {
    let mut p = libraries_dir()?;
    for segment in artifact_path.split('/') {
        p.push(segment);
    }
    Some(p)
}

/// Assets dir: `<shared>/assets`.
pub fn assets_dir() -> Option<PathBuf> {
    let mut p = shared()?;
    p.push("assets");
    Some(p)
}

/// Asset index path: `<assets>/indexes/<id>.json`.
pub fn asset_index_path(id: &str) -> Option<PathBuf> {
    let mut p = assets_dir()?;
    p.push("indexes");
    p.push(format!("{}.json", id));
    Some(p)
}

/// Asset object path: `<assets>/objects/<2-char>/<hash>`. Prefix is the
/// first 2 chars of the sha1, matching Mojang's CDN URL layout
/// (`resources.download.minecraft.net/<prefix>/<hash>`).
pub fn asset_object_path(hash: &str) -> Option<PathBuf> {
    if hash.len() < 2 {
        return None;
    }
    let prefix = &hash[..2];
    let mut p = assets_dir()?;
    p.push("objects");
    p.push(prefix);
    p.push(hash);
    Some(p)
}

/// Per-instance dir: `<root>/instances/<name>`. Worlds, screenshots,
/// per-instance config live here. Created lazily when an instance is
/// first launched (Phase C).
pub fn instance_dir(name: &str) -> Option<PathBuf> {
    let mut p = root()?;
    p.push("instances");
    p.push(name);
    Some(p)
}
