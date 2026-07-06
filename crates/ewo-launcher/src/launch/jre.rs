//! JRE detection — finds installed Java runtimes on the host so the
//! launcher can pick one matching the per-version manifest's
//! `javaVersion.majorVersion`.
//!
//! Scans well-known install paths (Oracle, Adoptium, Microsoft, Azul,
//! Liberica, plus `JAVA_HOME` and PATH), then runs `java -version` on
//! each candidate to read its major version. Caches the result in a
//! `OnceLock` so the scan only happens once per launcher process.
//!
//! Selection rule: prefer an exact major-version match; otherwise fall
//! back to the lowest installed major that's still ≥ the requirement.
//! No match → caller surfaces a clear error.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// One detected Java runtime — the absolute path to its `java` binary
/// + the major version it reports.
#[derive(Debug, Clone)]
pub struct DetectedJre {
    pub path: PathBuf,
    pub major: u32,
}

/// Lazy-initialized + invalidatable list of JREs the host has installed.
/// First call scans; subsequent calls return the cached vector. Call
/// `invalidate_cache` after a bundled JRE finishes downloading so it
/// gets picked up.
pub fn detect_all() -> Vec<DetectedJre> {
    let cache = cache_handle();
    {
        let guard = cache.lock().unwrap();
        if let Some(v) = guard.as_ref() {
            return v.clone();
        }
    }
    let scanned = scan();
    let mut guard = cache.lock().unwrap();
    *guard = Some(scanned.clone());
    scanned
}

/// Drop the cached scan so the next `detect_all` call re-scans. Used
/// after a bundled JRE download completes to add the freshly extracted
/// JRE to the candidate list.
pub fn invalidate_cache() {
    let cache = cache_handle();
    *cache.lock().unwrap() = None;
}

fn cache_handle() -> &'static Mutex<Option<Vec<DetectedJre>>> {
    static CACHE: OnceLock<Mutex<Option<Vec<DetectedJre>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Pick the best JRE for a required major version. Exact match wins;
/// otherwise the lowest installed major ≥ requirement; otherwise `None`.
pub fn pick_for_major(required: u32) -> Option<DetectedJre> {
    let all = detect_all();
    if let Some(exact) = all.iter().find(|j| j.major == required).cloned() {
        return Some(exact);
    }
    all.into_iter()
        .filter(|j| j.major >= required)
        .min_by_key(|j| j.major)
}

fn scan() -> Vec<DetectedJre> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    add_standard_paths(&mut candidates);
    add_path_lookup(&mut candidates);
    add_bundled_jres(&mut candidates);

    let mut out: Vec<DetectedJre> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for path in candidates {
        // De-dupe by canonical path so symlinks don't double-list.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.iter().any(|p| *p == canonical) {
            continue;
        }
        seen.push(canonical);
        if !path.exists() {
            continue;
        }
        if let Some(major) = read_java_major(&path) {
            log::info!("jre: detected Java {} at {}", major, path.display());
            out.push(DetectedJre { path, major });
        }
    }
    out.sort_by_key(|j| j.major);
    out
}

/// Run `java -version` on a candidate and parse the major-version number
/// out of the first stderr line (e.g.
/// `java version "21.0.8" 2025-07-15 LTS` → 21,
/// `openjdk version "1.8.0_392" ...` → 8).
fn read_java_major(java_exe: &Path) -> Option<u32> {
    let mut cmd = Command::new(java_exe);
    cmd.arg("-version");
    super::no_window(&mut cmd);
    let out = cmd.output().ok()?;
    // `java -version` writes to stderr by historical convention.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_major(&combined)
}

fn parse_major(version_text: &str) -> Option<u32> {
    // Find the quoted version string. Common shapes:
    //   `java version "21.0.8"` → 21
    //   `openjdk version "17.0.10"` → 17
    //   `java version "1.8.0_392"` → 8 (legacy 1.x form)
    //   `openjdk version "1.8.0_402"` → 8
    let start = version_text.find('"')?;
    let rest = &version_text[start + 1..];
    let end = rest.find('"')?;
    let v = &rest[..end];
    let mut parts = v.split('.');
    let first = parts.next()?.parse::<u32>().ok()?;
    if first == 1 {
        // Legacy `1.MAJOR.MINOR` form — second component is the major.
        let second = parts.next()?.parse::<u32>().ok()?;
        Some(second)
    } else {
        Some(first)
    }
}

fn add_path_lookup(out: &mut Vec<PathBuf>) {
    // `where java` returns multiple matches across PATH; we collect them all.
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("where");
        cmd.arg("java");
        super::no_window(&mut cmd);
        if let Ok(o) = cmd.output() {
            let text = String::from_utf8_lossy(&o.stdout);
            for line in text.lines() {
                let p = PathBuf::from(line.trim());
                if !p.as_os_str().is_empty() {
                    out.push(p);
                }
            }
        }
    } else if let Ok(o) = Command::new("which").arg("java").output() {
        let p = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string());
        if !p.as_os_str().is_empty() {
            out.push(p);
        }
    }
}

fn add_standard_paths(out: &mut Vec<PathBuf>) {
    if cfg!(target_os = "windows") {
        // Oracle "javapath" symlinks (auto-updated to the latest install
        // of that major). The user has these for both 8 and 21.
        out.push(PathBuf::from(
            r"C:\Program Files\Common Files\Oracle\Java\javapath\java.exe",
        ));
        out.push(PathBuf::from(
            r"C:\Program Files (x86)\Common Files\Oracle\Java\java8path\java.exe",
        ));
        // Oracle full installs — globbed below.
        for root in &[
            r"C:\Program Files\Java",
            r"C:\Program Files (x86)\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft\jdk",
            r"C:\Program Files\Zulu",
            r"C:\Program Files\BellSoft\LibericaJDK",
        ] {
            scan_versioned_subdirs(Path::new(root), out);
        }
    }
    // JAVA_HOME (cross-platform).
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let mut p = PathBuf::from(home);
        if cfg!(target_os = "windows") {
            p.push("bin");
            p.push("java.exe");
        } else {
            p.push("bin");
            p.push("java");
        }
        out.push(p);
    }
}

/// Scan an install root like `C:\Program Files\Java` for sub-dirs whose
/// `bin\java.exe` exists. Doesn't try to peek into the dir name; we let
/// the `java -version` step distinguish.
fn scan_versioned_subdirs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let mut p = entry.path();
        p.push("bin");
        p.push("java.exe");
        if p.exists() {
            out.push(p);
        }
    }
}

/// Scan our bundled-JRE dirs at
/// `<config>/EwoClient/runtime/<major>/jre/<archive-root>/bin/java.exe`.
/// The archive root is the top-level dir Adoptium ships (e.g.
/// `jdk-21.0.4+7-jre`); we don't know its name in advance, so we walk
/// one level deeper than the standard scan.
fn add_bundled_jres(out: &mut Vec<PathBuf>) {
    for jre_root in crate::runtime::paths::all_bundled_jre_roots() {
        scan_jre_root_recursive(&jre_root, out);
    }
}

/// Look one or two levels down for `bin/java[.exe]`. Adoptium archives
/// always have a single top-level dir, but we don't bake in its name.
fn scan_jre_root_recursive(root: &Path, out: &mut Vec<PathBuf>) {
    let java_exe = if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    };
    // Direct child layout: <root>/bin/java[.exe]
    {
        let direct = root.join("bin").join(java_exe);
        if direct.exists() {
            out.push(direct);
            return;
        }
    }
    // Nested-once layout: <root>/<archive-root>/bin/java[.exe]
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path().join("bin").join(java_exe);
            if p.exists() {
                out.push(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_major;

    #[test]
    fn parses_modern() {
        assert_eq!(
            parse_major(r#"java version "21.0.8" 2025-07-15 LTS"#),
            Some(21)
        );
    }

    #[test]
    fn parses_openjdk() {
        assert_eq!(
            parse_major(r#"openjdk version "17.0.10" 2024-01-16"#),
            Some(17)
        );
    }

    #[test]
    fn parses_legacy_1x() {
        assert_eq!(
            parse_major(r#"java version "1.8.0_392""#),
            Some(8)
        );
    }
}
