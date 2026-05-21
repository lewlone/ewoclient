//! Version manifest fetching + caching (v2 phase B).
//!
//! Fetches Mojang's `version_manifest_v2.json` so the launcher knows what
//! versions exist and where to find their per-version manifests. Cached on
//! disk at `<config>/EwoClient/versions_cache.json` with a 6-hour TTL.
//!
//! Endpoints:
//!   - <https://launchermeta.mojang.com/mc/game/version_manifest_v2.json>
//!   - per-version: each entry has its own `url` to a JSON manifest with
//!     client.jar, libraries, asset index, JVM args, etc. (Phase B-2.)
//!
//! No async runtime per CLAUDE.md non-negotiables — `ureq` for HTTP, a
//! background `std::thread` for the fetch, `mpsc` to deliver the parsed
//! manifest to the UI thread.

pub mod cache;
pub mod manifest;
pub mod per_version;
pub mod per_version_fetch;
pub mod service;

pub use service::VersionService;
