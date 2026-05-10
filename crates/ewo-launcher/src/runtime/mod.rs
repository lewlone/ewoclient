//! v2 phase C+ — bundled-JRE auto-fetch.
//!
//! When a Minecraft version's per-version manifest specifies a Java
//! `majorVersion` the host doesn't have, the launcher fetches Eclipse
//! Temurin's matching JRE from Adoptium and caches it at
//! `<config>/EwoClient/runtime/<major>/jre/`. The JRE detector
//! (`launch::jre`) scans both system installs and this dir, so once the
//! download completes the detector picks it up automatically.
//!
//! Triggers from `App::try_real_launch` when no installed JRE matches.
//! Reports progress through the launching screen so the user sees a
//! real progress bar during the fetch.

pub mod adoptium;
pub mod paths;
pub mod service;

pub use service::{RuntimeEvent, RuntimeService};
