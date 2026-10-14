//! File-system helpers shared by the launcher and the in-game cdylib: both
//! write config files the other side reads, so the write and the name rules
//! have to be the same code.
//!
//! These lived in `ewo-launcher::util`, which re-exports them; they moved here
//! so `ewo-core::pvp` (written by both the launcher and `ewo-jni`) can use them
//! without a dependency cycle.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Distinguishes the temp files of concurrent writes in one process.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically: write a temp file in the same
/// directory, fsync it, then rename over the target. A crash mid-write leaves
/// either the old file or the new one, never a truncated mix. Creates the
/// parent directory if missing.
///
/// On Windows the rename replaces the target in one step, but fails with
/// "access denied" while another process holds the target open without
/// `FILE_SHARE_DELETE` (an editor, a virus scanner, a mod re-reading it). That
/// is transient, so the rename is retried a few times before giving up.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write_with(path, contents, false)
}

/// [`atomic_write`] for a file holding credentials: on Unix the temp file is
/// *created* with mode 0600, so the secrets are never readable by another
/// user, not even between creation and a later `chmod`. (On Windows the
/// per-user `%APPDATA%` ACL already applies.)
pub fn atomic_write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write_with(path, contents, true)
}

/// The temp file's name, unique per write: two threads writing the same file
/// must not share a temp name, or one's rename moves the other's
/// half-written file.
fn temp_name(file_name: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut name = file_name.to_os_string();
    name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    name
}

/// Create the temp file; `private` makes it owner-only from the start.
#[cfg(unix)]
fn create_temp(tmp: &Path, private: bool) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if private {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(tmp)
    } else {
        fs::File::create(tmp)
    }
}

#[cfg(not(unix))]
fn create_temp(tmp: &Path, _private: bool) -> io::Result<fs::File> {
    fs::File::create(tmp)
}

fn atomic_write_with(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let tmp = parent.join(temp_name(file_name));
    let result = (|| {
        let mut f = create_temp(&tmp, private)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 5;
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < ATTEMPTS => {
                std::thread::sleep(Duration::from_millis(20 * u64::from(attempt)));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Why a user-supplied name can't be used as a directory / display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    /// `/ \ : * ? " < > |` or a control character.
    ForbiddenChar,
    /// `.`/`..`, a trailing dot or space, or a reserved Windows device name.
    Reserved,
}

/// Validate a name that may end up as a path component. Returns the trimmed
/// name on success. `max_chars` bounds the length in `char`s.
pub fn validate_name(name: &str, max_chars: usize) -> Result<&str, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > max_chars {
        return Err(NameError::TooLong);
    }
    if name
        .chars()
        .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return Err(NameError::ForbiddenChar);
    }
    if name == "." || name == ".." || name.ends_with('.') || name.contains("..") {
        return Err(NameError::Reserved);
    }
    // Windows reserves these device names, with or without an extension.
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        return Err(NameError::Reserved);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ewo-core-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn atomic_write_replaces_contents_and_leaves_no_temp() {
        let dir = scratch("atomic");
        let path = dir.join("nested").join("cfg.toml");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "cfg.toml")
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_write_gets_its_own_temp_name() {
        use std::collections::HashSet;
        let name = std::ffi::OsStr::new("pidfile");
        let names: HashSet<_> = (0..1000).map(|_| temp_name(name)).collect();
        assert_eq!(names.len(), 1000, "two writes shared a temp name");
        assert!(names
            .iter()
            .all(|n| n.to_string_lossy().starts_with("pidfile.tmp-")));
    }

    #[cfg(unix)]
    #[test]
    fn private_write_is_owner_only_from_creation_and_after_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("atomic-private");
        let path = dir.join("auth.toml");
        atomic_write_private(&path, b"secret").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        // A replacement of an (older, looser) file ends up private too.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write_private(&path, b"secret2").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret2");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn private_write_round_trips_on_every_platform() {
        let dir = scratch("atomic-private-rt");
        let path = dir.join("auth.toml");
        atomic_write_private(&path, b"one").unwrap();
        atomic_write_private(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_reports_a_failure_instead_of_swallowing_it() {
        let dir = scratch("atomic-err");
        fs::create_dir_all(&dir).unwrap();
        // A directory where the file should go: the rename cannot succeed.
        let path = dir.join("target");
        fs::create_dir_all(path.join("sub")).unwrap();
        assert!(atomic_write(&path, b"x").is_err());
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "temp file left behind: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_name_rejects_what_cannot_be_a_path_component() {
        assert_eq!(validate_name("Default", 48), Ok("Default"));
        assert_eq!(validate_name("  PvP 2 ", 48), Ok("PvP 2"));
        for bad in [
            "",
            "   ",
            "a/b",
            "a\\b",
            "C:",
            "..",
            "x..y",
            "trailing.",
            "con",
        ] {
            assert!(
                validate_name(bad, 48).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert_eq!(validate_name(&"x".repeat(49), 48), Err(NameError::TooLong));
    }
}
