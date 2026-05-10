//! Per-version manifest — the JSON Mojang serves at the URL listed in
//! each `version_manifest_v2.json` entry.
//!
//! Tells us:
//!   - Where to fetch `client.jar` (URL + sha1 + size)
//!   - Which libraries to download (URLs + sha1s + classifier rules)
//!   - Which asset index to fetch + how to apply it
//!   - The main class to invoke (e.g. `net.minecraft.client.main.Main`)
//!   - JVM + game args, with template tokens like `${auth_player_name}`
//!     left for Phase C to substitute
//!
//! Mojang changed the manifest format around 1.13 — this struct supports
//! both the modern `arguments` object and the legacy `minecraftArguments`
//! string (1.8.9 uses the legacy form). Unknown fields are ignored so
//! future Mojang additions don't break parsing.
//!
//! Ref: <https://wiki.vg/Game_files>

use serde::{Deserialize, Serialize};

/// Top-level per-version manifest. Spans both legacy (≤1.12) and modern
/// (1.13+) Minecraft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerVersion {
    pub id: String,
    /// JVM main class to invoke. e.g. `net.minecraft.client.main.Main`.
    #[serde(rename = "mainClass")]
    pub main_class: String,
    /// Asset index reference — points to the JSON that maps asset names
    /// to (hash, size). Always present on every version since 1.7.x.
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    /// Asset index ID, e.g. `"5"` or `"17"`. Same as `asset_index.id` —
    /// duplicated by Mojang for legacy reasons.
    #[serde(default)]
    pub assets: String,
    /// Required Java major version, e.g. `8`, `17`, `21`. Modern manifests
    /// only; absent on 1.8.9-era manifests.
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    /// File downloads (client.jar, optionally server.jar etc.). The
    /// `client` entry is the only one we actually need.
    pub downloads: Downloads,
    /// Libraries to put on the classpath (and unpack natives from).
    pub libraries: Vec<Library>,
    /// Modern (1.13+) argument blocks. Mutually exclusive with
    /// `minecraft_arguments` below.
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// Legacy (≤1.12) flat argument string. e.g. for 1.8.9 it's
    /// `"--username ${auth_player_name} --version ${version_name} ..."`.
    /// Mutually exclusive with `arguments` above.
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    /// ISO 8601 release timestamp.
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
    /// Schema-version field Mojang occasionally bumps. Modern is `21121`
    /// (June 2021); we don't currently gate on this.
    #[serde(rename = "complianceLevel", default)]
    pub compliance_level: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    /// Total uncompressed size of all assets — useful for progress UI
    /// before we've enumerated the asset index itself.
    #[serde(rename = "totalSize", default)]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaVersion {
    /// Mojang's runtime component name. e.g. `java-runtime-gamma` for J17,
    /// `java-runtime-delta` for J21. Used as a key into the JRE manifest
    /// when bundling Adoptium runtimes (Phase C).
    pub component: String,
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Downloads {
    pub client: ClientDownload,
    /// Server jar — present on most versions, but we never download it.
    /// Kept for completeness; ignored at runtime.
    #[serde(default)]
    pub server: Option<ClientDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientDownload {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// One library on the classpath, plus optional rules + native artifacts.
///
/// Mojang represents libraries in two slightly different formats across
/// the format change around 1.13:
///   - **Modern** (1.13+): `downloads.artifact` is the main jar;
///     `downloads.classifiers.<native-key>` is the native bundle (also
///     a jar containing platform DLLs/SOs to extract).
///   - **Legacy** (≤1.12): same shape, `natives` map gives the classifier
///     key per OS (e.g. `"natives-windows"` → look up
///     `downloads.classifiers["natives-windows"]`).
///
/// Both shapes parse fine into this struct because we just preserve the
/// raw `downloads` block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    /// Maven coordinates: `group:artifact:version[:classifier]`.
    /// e.g. `org.lwjgl:lwjgl:3.3.3` or
    /// `org.lwjgl:lwjgl:3.3.3:natives-windows`.
    pub name: String,
    pub downloads: LibraryDownloads,
    /// Per-OS classifier mapping for native libs (legacy format).
    /// On modern manifests this is omitted — natives go into
    /// `downloads.classifiers` directly.
    #[serde(default)]
    pub natives: std::collections::HashMap<String, String>,
    /// Allow/disallow rules. If absent, library always applies.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Files inside the natives jar to skip when extracting (typically
    /// META-INF). Modern manifests put this under `extract.exclude`.
    #[serde(default)]
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryDownloads {
    /// Main jar artifact. `None` for libraries that are pure native
    /// bundles (no Java code, only platform binaries).
    #[serde(default)]
    pub artifact: Option<Artifact>,
    /// Per-classifier artifacts (natives, sources, etc.).
    #[serde(default)]
    pub classifiers: std::collections::HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Path *relative to the libraries dir*, e.g.
    /// `org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar`. We mirror this on disk
    /// under `<config>/EwoClient/shared/libraries/`.
    pub path: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Allow/disallow rule on a library or argument. Phase B-3 adds the
/// evaluator; for now we just preserve the raw shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsCondition>,
    #[serde(default)]
    pub features: Option<std::collections::HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsCondition {
    /// `windows`, `osx`, `linux`. Absent → matches any OS.
    #[serde(default)]
    pub name: Option<String>,
    /// Architecture, e.g. `x86`, `arm64`. Rare; mostly absent.
    #[serde(default)]
    pub arch: Option<String>,
    /// Version regex (literal, not regex pattern). e.g. `^10\\.` for
    /// "Windows 10+". Rare; mostly absent.
    #[serde(default)]
    pub version: Option<String>,
}

/// Modern (1.13+) argument block. Each entry is either a plain string or
/// a conditional `{rules, value}` object — `serde_json::Value` keeps
/// things flexible without enumerating every possibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}
