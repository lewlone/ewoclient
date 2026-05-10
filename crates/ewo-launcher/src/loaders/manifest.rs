//! Loader profile JSON — what a mod-loader's per-version endpoint returns.
//!
//! Real-world example: Fabric serves
//! `https://meta.fabricmc.net/v2/versions/loader/<mc>/<loader>/profile/json`,
//! which returns this exact shape. The synthetic `id`
//! (`"fabric-loader-0.16.10-1.21.4"`) is what shows up in the launcher
//! profile list; `inherits_from` names the vanilla version it layers on.
//!
//! A `LoaderManifest` is parsed from JSON, then `merge::merge` combines
//! it with a vanilla `PerVersion` to produce a final manifest the launch
//! pipeline consumes unchanged.
//!
//! Unknown fields are ignored so future loader manifests with extra keys
//! parse fine.

use serde::{Deserialize, Serialize};

use crate::versions::per_version::{Arguments, Library};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderManifest {
    /// Synthetic profile ID, e.g. `"fabric-loader-0.16.10-1.21.4"`.
    pub id: String,
    /// Vanilla MC version this loader applies on top of, e.g. `"1.21.4"`.
    #[serde(rename = "inheritsFrom")]
    pub inherits_from: String,
    /// Replaces vanilla's `mainClass` when present. Fabric uses
    /// `"net.fabricmc.loader.impl.launch.knot.KnotClient"`.
    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
    /// Libraries the loader needs on the classpath. Merged ahead of
    /// vanilla's so loader patches take precedence.
    #[serde(default)]
    pub libraries: Vec<Library>,
    /// Additional JVM + game args to prepend to vanilla's.
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(rename = "releaseTime", default)]
    pub release_time: Option<String>,
}
