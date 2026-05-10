//! On-disk paths for bundled JREs.
//!
//! `<config>/EwoClient/runtime/<major>/jre/<top-level>/bin/java.exe`
//!
//! `<top-level>` is the dir name from the Adoptium archive (e.g.
//! `jdk-21.0.4+7-jre`). We don't try to flatten — we leave the archive's
//! native layout intact and probe `bin/java.exe` recursively when
//! detecting installed JREs.

use std::path::PathBuf;

/// `<config>/EwoClient/runtime/<major>`.
pub fn runtime_dir(major: u32) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("runtime");
    p.push(major.to_string());
    Some(p)
}

/// Root scan dir for the JRE detector — returns
/// `<config>/EwoClient/runtime/<major>/jre`. Detector walks one level
/// deep looking for `bin/java.exe`.
pub fn jre_dir(major: u32) -> Option<PathBuf> {
    let mut p = runtime_dir(major)?;
    p.push("jre");
    Some(p)
}

/// All `<config>/EwoClient/runtime/<major>/jre/` dirs that exist. The
/// JRE detector adds these to its candidate scan.
pub fn all_bundled_jre_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(mut runtime) = dirs::config_dir() else {
        return out;
    };
    runtime.push("EwoClient");
    runtime.push("runtime");
    if let Ok(entries) = std::fs::read_dir(&runtime) {
        for entry in entries.flatten() {
            let mut p = entry.path();
            p.push("jre");
            if p.exists() {
                out.push(p);
            }
        }
    }
    out
}
