//! v2 phase D — mod-loader manifest support.
//!
//! A loader (Fabric, the upcoming custom Fabric fork, etc.) ships a
//! per-version JSON profile that layers on top of vanilla: it overrides
//! `mainClass`, prepends extra libraries to the classpath, and prepends
//! extra JVM/game args. We model it as `LoaderManifest`, then `merge`
//! produces a final `PerVersion` that the existing `launch::build`
//! consumes unchanged.
//!
//! Fetching lives in `fetch` — supports HTTP and `file://` so the
//! in-development EwoLoader manifest can be consumed directly off disk.

pub mod fetch;
pub mod manifest;
pub mod merge;

pub use fetch::get_or_fetch;
pub use merge::merge;

/// What the download job needs to know about a loader: a logical id (for
/// cache keying + log labels) and the manifest URL to fetch. Constructed
/// from an `InstanceLoader::Ewo { manifest_url }` at the launcher boundary
/// so neither `downloads` nor `loaders` depends on `ewo-render`'s model.
#[derive(Debug, Clone)]
pub struct LoaderSpec {
    pub id: String,
    pub url: String,
}
