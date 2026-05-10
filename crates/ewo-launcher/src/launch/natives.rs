//! Native-library extraction.
//!
//! Some libraries ship as JARs containing platform binaries (`.dll` on
//! Windows, `.so` on Linux, `.dylib` on macOS) that the JVM loads via
//! `System.loadLibrary` from a path passed as `-Djava.library.path=...`.
//!
//! Before launching, we unzip each library's native classifier JAR into
//! the per-instance `natives/` dir, skipping files matched by the lib's
//! `extract.exclude` list (typically `META-INF/`).
//!
//! Re-extracting on every launch is fine — the dir is small (~10MB) and
//! it picks up updated lib versions if the user re-downloads.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::downloads::paths;
use crate::versions::per_version::{Library, PerVersion};

use super::plan::pick_native_classifier;

#[derive(Debug, Clone)]
pub enum ExtractError {
    Disk(String),
    Zip(String),
    PathsUnresolvable,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Disk(s) => write!(f, "disk: {}", s),
            ExtractError::Zip(s) => write!(f, "zip: {}", s),
            ExtractError::PathsUnresolvable => write!(f, "config dir unresolvable"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// Extract every applicable native classifier into the instance's
/// `natives/` dir. Wipes the dir first so stale natives don't persist.
pub fn extract_all(pv: &PerVersion, instance_name: &str) -> Result<(), ExtractError> {
    let dest = super::plan::natives_dir_for(instance_name)
        .ok_or(ExtractError::PathsUnresolvable)?;
    if dest.exists() {
        fs::remove_dir_all(&dest)
            .map_err(|e| ExtractError::Disk(format!("rm {}: {}", dest.display(), e)))?;
    }
    fs::create_dir_all(&dest)
        .map_err(|e| ExtractError::Disk(format!("mkdir {}: {}", dest.display(), e)))?;

    for lib in &pv.libraries {
        if !crate::downloads::rules::rules_pass(&lib.rules) {
            continue;
        }
        let Some(classifier) = pick_native_classifier(lib) else {
            continue;
        };
        let Some(art) = lib.downloads.classifiers.get(&classifier) else {
            continue;
        };
        let Some(jar_path) = paths::library_path(&art.path) else {
            continue;
        };
        if !jar_path.exists() {
            log::warn!(
                "natives: jar missing for {} ({}): {}",
                lib.name,
                classifier,
                jar_path.display()
            );
            continue;
        }
        let excludes = lib
            .extract
            .as_ref()
            .map(|e| e.exclude.as_slice())
            .unwrap_or(&[]);
        extract_one(&jar_path, &dest, excludes)?;
    }
    Ok(())
}

fn extract_one(jar: &Path, dest: &Path, excludes: &[String]) -> Result<(), ExtractError> {
    let f = fs::File::open(jar)
        .map_err(|e| ExtractError::Disk(format!("open {}: {}", jar.display(), e)))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| ExtractError::Zip(e.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ExtractError::Zip(e.to_string()))?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        // Skip excluded paths (typically META-INF/).
        if excludes.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        // Strip path separators that would put files outside `dest`.
        // tiny defense against path traversal in malicious zips. Mojang's
        // own jars are trustworthy but it's cheap to be paranoid.
        let safe_name = name.replace('\\', "/");
        if safe_name.contains("..") {
            continue;
        }
        let out_path: PathBuf = dest.join(&safe_name);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ExtractError::Disk(format!("mkdir {}: {}", parent.display(), e)))?;
        }
        let mut out = fs::File::create(&out_path)
            .map_err(|e| ExtractError::Disk(format!("create {}: {}", out_path.display(), e)))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(io_to_zip)?;
            if n == 0 {
                break;
            }
            io::Write::write_all(&mut out, &buf[..n]).map_err(io_to_zip)?;
        }
    }
    Ok(())
}

fn io_to_zip(e: io::Error) -> ExtractError {
    ExtractError::Zip(e.to_string())
}

/// Convenience helper for libraries without their own native classifier
/// — returns whether the lib has one applicable on this host.
pub fn has_native(lib: &Library) -> bool {
    pick_native_classifier(lib).is_some()
}
