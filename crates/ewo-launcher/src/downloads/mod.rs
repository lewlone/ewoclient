//! v2 phase B — real downloads.
//!
//! Given a master manifest entry, fetches everything needed to launch
//! that version: per-version manifest, client jar, libraries (with
//! native classifiers + OS rules), asset index, all asset blobs.
//! sha1-verified end to end. Mirrors Mojang's official launcher's disk
//! layout so installations are interoperable.
//!
//! Background-threaded; UI thread polls a `DownloadService` for events.

pub mod asset_index;
pub mod job;
pub mod paths;
pub mod rules;
pub mod service;

pub use job::ensure_libraries;
pub use service::DownloadService;
