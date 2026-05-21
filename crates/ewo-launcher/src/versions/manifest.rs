//! Mojang `version_manifest_v2.json` types.
//!
//! Only the fields the launcher actually uses today. Mojang adds new
//! fields over time (compliance, server URLs, etc.); we ignore unknown
//! fields by default so future additions don't break the parse.

use serde::{Deserialize, Serialize};

/// Public URL of the master version manifest. Mojang has kept this stable
/// for years; if it ever moves, swap here.
pub const VERSION_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

/// One entry from `versions[]` in the master manifest. `url` points to a
/// per-version manifest with client.jar, libraries, asset index, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ManifestEntryKind,
    pub url: String,
    /// ISO 8601 timestamp the version was released.
    #[serde(default)]
    pub release_time: String,
    /// SHA-1 of the per-version manifest at `url`. Used to detect cache
    /// staleness without re-fetching the per-version body.
    #[serde(default)]
    pub sha1: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryKind {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

/// The master manifest as returned by `version_manifest_v2.json`. The
/// `latest` block names the current Mojang-recommended release + snapshot,
/// independent of ordering in `versions[]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

impl VersionManifest {
    /// Find an entry by ID. Used when the user selects a version to launch
    /// — we look up the URL of its per-version manifest.
    pub fn entry(&self, id: &str) -> Option<&ManifestEntry> {
        self.versions.iter().find(|e| e.id == id)
    }

    /// Filter the version list to the launcher's supported set. The
    /// supported set is intentionally narrow — this launcher targets
    /// 1.21.x + 26.x + 1.8.9 specifically, not the entire Mojang
    /// catalogue. Anything outside this set is filtered out.
    ///
    /// (Decision recorded in CLAUDE.md → v2 Phase B; the "support more
    /// versions" knob is a future scope item, not a current toggle.)
    pub fn filtered_for_dropdown(&self, _include_snapshots: bool) -> Vec<&ManifestEntry> {
        self.versions.iter().filter(|e| is_supported(e)).collect()
    }
}

/// Whether a manifest entry is in the launcher's curated supported set.
///
/// Supported (releases): the 1.21.x line, the 26.x line, plus 1.8.9.
/// Supported (snapshots): only `26.2-snapshot-5` — explicit allowlist.
/// All other snapshots, pre-releases, release candidates, weekly
/// snapshots, old-beta, and old-alpha are excluded.
///
/// To allow another snapshot, add its exact ID to `SNAPSHOT_ALLOWLIST`.
pub fn is_supported(entry: &ManifestEntry) -> bool {
    let id = entry.id.as_str();
    if id == "1.8.9" {
        return true;
    }
    let is_121 = id == "1.21" || id.starts_with("1.21.");
    let is_26_release_line = id == "26" || id.starts_with("26.");
    match entry.kind {
        ManifestEntryKind::Release => is_121 || is_26_release_line,
        ManifestEntryKind::Snapshot => SNAPSHOT_ALLOWLIST.contains(&id),
        ManifestEntryKind::OldBeta | ManifestEntryKind::OldAlpha => false,
    }
}

/// Exact-match allowlist of snapshot IDs to surface in the dropdown.
/// Add or remove entries here to change which snapshots users can pick.
const SNAPSHOT_ALLOWLIST: &[&str] = &["26.2-snapshot-5"];
