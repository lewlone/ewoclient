//! Small launcher-wide helpers that don't belong to any one subsystem.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Convert a `file://` URL into a filesystem path, or return `None` for a
/// non-`file://` URL (or one whose percent-escapes don't decode to UTF-8).
///
/// Handles both unix-style `file:///home/user/x.jar` → `/home/user/x.jar`
/// and Windows-style `file:///C:/path/to.jar` → `C:/path/to.jar` (the
/// leading `/` before the drive letter is stripped). Percent-escapes are
/// decoded to bytes first and the result is then read as UTF-8, so
/// non-ASCII paths round-trip. Authority and query strings aren't handled —
/// loader/library URLs don't use them.
pub fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Strip the leading `/` on Windows-style `file:///C:/...` so the
    // result becomes `C:/...` (a real drive-letter path), not `/C:/...`.
    let trimmed = if cfg!(windows)
        && rest.starts_with('/')
        && rest.len() >= 3
        && rest.as_bytes()[1].is_ascii_alphabetic()
        && rest.as_bytes()[2] == b':'
    {
        &rest[1..]
    } else {
        rest
    };
    let decoded = percent_decode(trimmed)?;
    Some(PathBuf::from(decoded))
}

/// Decode `%XX` escapes into raw bytes, then validate as UTF-8. A `%` not
/// followed by two hex digits is kept literally.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Write `contents` to `path` atomically: write a temp file in the same
/// directory, fsync it, then rename over the target. A crash mid-write leaves
/// either the old file or the new one, never a truncated mix. Creates the
/// parent directory if missing.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = parent.join(tmp_name);
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// [`atomic_write`] for a file holding credentials: on Unix the result is
/// mode 0600. (On Windows the per-user `%APPDATA%` ACL already applies.)
pub fn atomic_write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Preserve a config file that failed to parse by copying it to
/// `<name>.bak-<unix_ts>` before any save can overwrite it. Logs an error
/// either way; returns the backup path on success.
pub fn backup_unparseable(path: &Path, reason: &dyn std::fmt::Display) -> Option<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".bak-{ts}"));
    let backup = path.with_file_name(name);
    match fs::copy(path, &backup) {
        Ok(_) => {
            log::error!(
                "could not parse {}: {} — preserved a copy at {}; using defaults",
                path.display(),
                reason,
                backup.display()
            );
            Some(backup)
        }
        Err(e) => {
            log::error!(
                "could not parse {}: {} — and backing it up to {} failed: {}",
                path.display(),
                reason,
                backup.display(),
                e
            );
            None
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
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
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

    #[test]
    fn file_url_decodes_ascii_and_escapes() {
        let p = file_url_to_path("file:///tmp/a%20b.jar").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/a b.jar"));
        assert!(file_url_to_path("https://x/y").is_none());
    }

    #[test]
    fn file_url_decodes_trailing_escape() {
        // Off-by-one regression: a `%XX` at the very end must decode.
        let p = file_url_to_path("file:///tmp/x%41").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/xA"));
    }

    #[test]
    fn file_url_decodes_utf8_escapes_and_literal_non_ascii() {
        // `ä` = C3 A4 in UTF-8.
        let p = file_url_to_path("file:///tmp/k%C3%A4ytt%C3%A4j%C3%A4").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/käyttäjä"));
        // Raw (unescaped) non-ASCII must not be mangled either.
        let p = file_url_to_path("file:///tmp/käyttäjä").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/käyttäjä"));
    }

    #[test]
    fn file_url_keeps_malformed_escape_and_rejects_invalid_utf8() {
        let p = file_url_to_path("file:///tmp/100%zz").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/100%zz"));
        let p = file_url_to_path("file:///tmp/50%").unwrap();
        assert!(p.to_string_lossy().ends_with("tmp/50%"));
        assert!(file_url_to_path("file:///tmp/%FF").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn file_url_strips_slash_before_drive_letter() {
        assert_eq!(
            file_url_to_path("file:///C:/a/b.jar").unwrap(),
            PathBuf::from("C:/a/b.jar")
        );
    }

    #[test]
    fn atomic_write_replaces_contents_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("ewo-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
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
        let bak = backup_unparseable(&path, &"test").unwrap();
        assert_eq!(fs::read_to_string(&bak).unwrap(), "two");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_name_accepts_ordinary_names() {
        assert_eq!(validate_name("  Velvet Hours ", 48), Ok("Velvet Hours"));
        assert_eq!(validate_name("käyttäjä's world 2", 48), Ok("käyttäjä's world 2"));
        assert_eq!(validate_name("v1.21", 48), Ok("v1.21"));
    }

    #[test]
    fn validate_name_rejects_path_tricks() {
        assert_eq!(validate_name("", 48), Err(NameError::Empty));
        assert_eq!(validate_name("   ", 48), Err(NameError::Empty));
        assert_eq!(validate_name("a/b", 48), Err(NameError::ForbiddenChar));
        assert_eq!(validate_name("a\\b", 48), Err(NameError::ForbiddenChar));
        assert_eq!(validate_name("C:", 48), Err(NameError::ForbiddenChar));
        assert_eq!(validate_name("a\u{7}b", 48), Err(NameError::ForbiddenChar));
        assert_eq!(validate_name("..", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("x..y", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("trailing.", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("con", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("NUL.txt", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("com1", 48), Err(NameError::Reserved));
        assert_eq!(validate_name("com0", 48), Ok("com0"));
        assert_eq!(validate_name("console", 48), Ok("console"));
        assert_eq!(validate_name(&"x".repeat(49), 48), Err(NameError::TooLong));
    }
}
