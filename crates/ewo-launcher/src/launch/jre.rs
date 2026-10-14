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

use std::ffi::OsStr;
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

/// Re-scan on a worker thread and swap the result into the cache, then
/// send `tag` on the returned channel. Used after a bundled JRE download:
/// the scan runs `java -version` per candidate and `where java`, which
/// must not stall the UI thread. The old list stays readable until the
/// new one is swapped in (no invalidate-then-rescan window in which a
/// UI-thread `pick_for_major` would scan cold).
pub fn rescan_in_background(tag: u32) -> std::sync::mpsc::Receiver<u32> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ewo-jre-rescan".into())
        .spawn(move || {
            let scanned = scan();
            *cache_handle().lock().unwrap() = Some(scanned);
            let _ = tx.send(tag);
        });
    if let Err(e) = spawned {
        // No thread: fall back to a lazy rescan on the next lookup.
        log::warn!("jre: could not spawn the rescan thread ({e}); rescanning lazily");
        invalidate_cache();
    }
    rx
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

/// One runtime's dropdown label. Pure so the UI string and its unit
/// test can't drift (the versioned install dir reads better than "bin";
/// anything unexpected degrades to the binary's own path).
fn label_for(j: &DetectedJre) -> String {
    let dir = j
        .path
        .parent()
        .and_then(Path::parent)
        .and_then(|p| p.file_name())
        .and_then(OsStr::to_str);
    match dir {
        Some(d) => format!("Java {} · {}", j.major, d),
        None => format!("Java {} · {}", j.major, j.path.display()),
    }
}

/// Dropdown-facing labels for every detected runtime, index-aligned with
/// `detect_all()`'s output (sorted by major ascending). Index 0 of the
/// UI's option list is the "Auto" pseudo-entry and lives caller-side;
/// this starts at the first real runtime.
///
/// Reads the cached scan, so call it from a worker, or once the cache is
/// warm: a cold `detect_all` shells `java -version` per candidate. The
/// strings come from disk paths, so they are only stable while the
/// installed-JRE set is. A persisted `Instance::java_runtime` pin that
/// points past the current list, or at a runtime that no longer meets
/// the version's requirement, falls back to auto at launch time (see
/// [`resolve_pin`]).
pub fn labels() -> Vec<String> {
    detect_all().iter().map(label_for).collect()
}

/// Resolve a per-instance Java-runtime pin against a detected list.
/// `pin` is the dropdown index: `0` = auto (returns `None`), `N >= 1`
/// selects `all[N - 1]`. A pin is honoured only while it still exists
/// and still satisfies `required`; otherwise `None`, so the caller falls
/// back to [`pick_for_major`]. A stale pin logs a warning.
pub fn resolve_pin(all: &[DetectedJre], pin: usize, required: u32) -> Option<DetectedJre> {
    if pin == 0 {
        return None;
    }
    let Some(j) = all.get(pin - 1) else {
        log::warn!(
            "launch: pinned Java runtime #{pin} is no longer installed ({} detected) - falling back to auto",
            all.len()
        );
        return None;
    };
    if j.major < required {
        log::warn!(
            "launch: pinned Java {} can't satisfy required >= {} - falling back to auto",
            j.major,
            required
        );
        return None;
    }
    Some(j.clone())
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
        if seen.contains(&canonical) {
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
    use super::{label_for, parse_major, resolve_pin, DetectedJre};
    use std::path::PathBuf;

    fn jre(major: u32, dir: &str) -> DetectedJre {
        DetectedJre {
            path: PathBuf::from(format!(r"C:\jvm\{dir}\bin\java.exe")),
            major,
        }
    }

    #[test]
    fn labels_use_the_versioned_install_dir() {
        let j = DetectedJre {
            path: PathBuf::from(r"C:\Program Files\Eclipse Adoptium\jdk-21.0.8+7\bin\java.exe"),
            major: 21,
        };
        assert_eq!(label_for(&j), r"Java 21 · jdk-21.0.8+7");
    }

    #[test]
    fn label_degrades_to_the_path_without_an_install_dir() {
        let j = DetectedJre {
            path: PathBuf::from("java"),
            major: 17,
        };
        assert_eq!(label_for(&j), "Java 17 · java");
    }

    #[test]
    fn pin_zero_is_auto() {
        let all = [jre(17, "a"), jre(21, "b")];
        assert!(resolve_pin(&all, 0, 17).is_none());
    }

    #[test]
    fn pin_selects_the_list_entry_one_behind_the_dropdown_index() {
        let all = [jre(17, "a"), jre(21, "b")];
        assert_eq!(resolve_pin(&all, 1, 17).unwrap().major, 17);
        assert_eq!(resolve_pin(&all, 2, 17).unwrap().major, 21);
    }

    #[test]
    fn stale_pin_past_the_list_falls_back_to_auto() {
        let all = [jre(21, "b")];
        assert!(resolve_pin(&all, 3, 17).is_none());
        assert!(resolve_pin(&[], 1, 8).is_none());
    }

    #[test]
    fn pin_below_the_required_major_falls_back_to_auto() {
        let all = [jre(17, "a"), jre(21, "b")];
        assert!(resolve_pin(&all, 1, 21).is_none());
    }

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
        assert_eq!(parse_major(r#"java version "1.8.0_392""#), Some(8));
    }
}
