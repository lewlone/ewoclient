//! Adoptium API client — fetches Eclipse Temurin JRE archives.
//!
//! Endpoint: <https://api.adoptium.net/v3/assets/feature_releases/{major}/ga>
//!
//! Filters narrow to "current host" (x64 Windows / x64 Linux / x64 macOS),
//! `image_type=jre` (smaller than full JDK), normal heap. Returns a
//! `.zip` (Windows) or `.tar.gz` (Unix) archive URL + sha256.

use std::time::Duration;

use serde::Deserialize;

/// Adoptium feature-release endpoint. `{major}` is the Java major version
/// (`8`, `17`, `21`, `25`, ...).
fn feature_releases_url(major: u32) -> String {
    format!(
        "https://api.adoptium.net/v3/assets/feature_releases/{}/ga\
         ?architecture={arch}&heap_size=normal&image_type=jre\
         &os={os}&page=0&page_size=1\
         &sort_method=DEFAULT&sort_order=DESC&vendor=eclipse",
        major,
        arch = host_arch(),
        os = host_os(),
    )
}

fn host_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac",
        "linux" => "linux",
        other => other,
    }
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => other,
    }
}

/// Adoptium response shape (only the fields we use).
#[derive(Debug, Clone, Deserialize)]
struct FeatureRelease {
    binaries: Vec<Binary>,
    /// `release_name` / `version_data.openjdk_version` give us a
    /// human-readable label like "jdk-21.0.4+7"; useful for log lines.
    #[serde(default)]
    release_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Binary {
    package: BinaryPackage,
}

#[derive(Debug, Clone, Deserialize)]
struct BinaryPackage {
    /// Direct download URL (ZIP on Windows, tar.gz on Unix).
    link: String,
    /// SHA-256 of the archive. We verify before extraction.
    checksum: String,
    /// Archive byte size — used for the progress bar denominator.
    size: u64,
    /// Filename inside the archive prefix, e.g.
    /// `OpenJDK21U-jre_x64_windows_hotspot_21.0.4_7.zip`. We use this
    /// as the on-disk archive name in the cache dir.
    name: String,
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub archive_name: String,
    pub release_name: String,
}

#[derive(Debug, Clone)]
pub enum AdoptiumError {
    Network(String),
    Parse(String),
    NotAvailable(u32),
}

impl std::fmt::Display for AdoptiumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdoptiumError::Network(s) => write!(f, "network: {}", s),
            AdoptiumError::Parse(s) => write!(f, "parse: {}", s),
            AdoptiumError::NotAvailable(major) => {
                write!(f, "no Adoptium JRE for Java {} on this host", major)
            }
        }
    }
}

impl std::error::Error for AdoptiumError {}

/// Look up the latest GA Adoptium JRE for `major` on this host.
pub fn latest_jre(major: u32) -> Result<ReleaseInfo, AdoptiumError> {
    let url = feature_releases_url(major);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    let body: Vec<FeatureRelease> = match agent.get(&url).call() {
        Ok(r) => r
            .into_json()
            .map_err(|e| AdoptiumError::Parse(format!("body: {}", e)))?,
        Err(ureq::Error::Status(404, _)) => return Err(AdoptiumError::NotAvailable(major)),
        Err(ureq::Error::Status(code, resp)) => {
            return Err(AdoptiumError::Network(format!(
                "{} {}",
                code,
                resp.into_string().unwrap_or_default()
            )));
        }
        Err(e) => return Err(AdoptiumError::Network(e.to_string())),
    };
    let release = body
        .into_iter()
        .next()
        .ok_or(AdoptiumError::NotAvailable(major))?;
    let binary = release
        .binaries
        .into_iter()
        .next()
        .ok_or(AdoptiumError::NotAvailable(major))?;
    Ok(ReleaseInfo {
        url: binary.package.link,
        sha256: binary.package.checksum,
        size: binary.package.size,
        archive_name: binary.package.name,
        release_name: release.release_name,
    })
}
