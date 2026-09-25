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
use std::path::{Component, Path, PathBuf};

use crate::downloads::paths;
use crate::versions::per_version::PerVersion;

use super::plan::pick_native_classifier;

#[derive(Debug, Clone)]
pub enum ExtractError {
    Disk(String),
    /// A file in the natives dir is held open by another process (sharing
    /// violation / lock / a mapped DLL that can't be deleted). The only case
    /// where reaping a lingering game process can help.
    InUse(String),
    Zip(String),
    PathsUnresolvable,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Disk(s) => write!(f, "disk: {}", s),
            ExtractError::InUse(s) => write!(f, "file in use: {}", s),
            ExtractError::Zip(s) => write!(f, "zip: {}", s),
            ExtractError::PathsUnresolvable => write!(f, "config dir unresolvable"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// True when `e` means "another process has this file open": Windows
/// `ERROR_ACCESS_DENIED` (deleting a mapped DLL), `ERROR_SHARING_VIOLATION`,
/// `ERROR_LOCK_VIOLATION`. Always false off Windows.
fn is_in_use(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

fn disk_err(what: String, e: io::Error) -> ExtractError {
    if is_in_use(&e) {
        ExtractError::InUse(format!("{what}: {e}"))
    } else {
        ExtractError::Disk(format!("{what}: {e}"))
    }
}

/// Extract every applicable native classifier into the instance's
/// `natives/` dir. Wipes the dir first so stale natives don't persist.
pub fn extract_all(pv: &PerVersion, instance_id: &str) -> Result<(), ExtractError> {
    let dest = super::plan::natives_dir_for(instance_id)
        .ok_or(ExtractError::PathsUnresolvable)?;
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| disk_err(format!("rm {}", dest.display()), e))?;
    }
    fs::create_dir_all(&dest).map_err(|e| disk_err(format!("mkdir {}", dest.display()), e))?;

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

/// A relative archive-entry path that stays inside the extraction root:
/// only normal components (and `.`), no root, drive prefix or `..`.
pub(crate) fn is_contained_relative(path: &Path) -> bool {
    let mut any = false;
    for c in path.components() {
        match c {
            Component::Normal(_) => any = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    any
}

/// The destination-relative path for a zip entry, or `None` for anything
/// that could escape `dest` or isn't a plain file (symlinks, dirs).
pub(crate) fn safe_zip_entry_path(entry: &zip::read::ZipFile<'_>) -> Option<PathBuf> {
    if entry.is_dir() || entry.is_symlink() {
        return None;
    }
    // `enclosed_name` rejects absolute paths, NULs and escaping `..`; we
    // additionally refuse any `..` component at all.
    let rel = entry.enclosed_name()?;
    is_contained_relative(&rel).then_some(rel)
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
        let Some(rel) = safe_zip_entry_path(&entry) else {
            log::warn!("natives: skipping unsafe entry {:?} in {}", name, jar.display());
            continue;
        };
        let out_path = dest.join(rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| disk_err(format!("mkdir {}", parent.display()), e))?;
        }
        let mut out = fs::File::create(&out_path)
            .map_err(|e| disk_err(format!("create {}", out_path.display()), e))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn contained_relative_paths() {
        assert!(is_contained_relative(Path::new("lwjgl.dll")));
        assert!(is_contained_relative(Path::new("sub/dir/x.so")));
        assert!(is_contained_relative(Path::new("./x.so")));
        assert!(!is_contained_relative(Path::new("")));
        assert!(!is_contained_relative(Path::new("../x.dll")));
        assert!(!is_contained_relative(Path::new("a/../../x.dll")));
        assert!(!is_contained_relative(Path::new("a/../x.dll")));
        assert!(!is_contained_relative(Path::new("/etc/x")));
        #[cfg(windows)]
        {
            assert!(!is_contained_relative(Path::new("C:\\x.dll")));
            assert!(!is_contained_relative(Path::new("C:x.dll")));
            assert!(!is_contained_relative(Path::new("\\x.dll")));
        }
    }

    #[test]
    fn extract_one_skips_escaping_entries() {
        let base = std::env::temp_dir().join(format!("ewo-natives-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dest = base.join("natives");
        fs::create_dir_all(&dest).unwrap();
        let jar = base.join("natives.jar");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for name in ["ok.dll", "../escaped.dll", "/abs.dll", "META-INF/x.sf", "sub/n.so"] {
                w.start_file(name, opts).unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
        }
        extract_one(&jar, &dest, &["META-INF/".to_string()]).unwrap();
        assert!(dest.join("ok.dll").exists());
        assert!(dest.join("sub").join("n.so").exists());
        assert!(!base.join("escaped.dll").exists());
        assert!(!dest.join("META-INF").exists());
        let _ = fs::remove_dir_all(&base);
    }
}
