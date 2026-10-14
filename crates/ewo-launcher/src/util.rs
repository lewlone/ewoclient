//! Small launcher-wide helpers that don't belong to any one subsystem.

use std::fs;
use std::io;
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

/// Inverse of [`file_url_to_path`]: build a `file://` URL from a filesystem
/// path. Backslashes become `/`, a Windows drive path gets the extra leading
/// `/` (`file:///C:/x`), and only the characters `file_url_to_path` would
/// misread (`%`, space, `#`, `?`) are percent-escaped.
pub fn path_to_file_url(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/');
    }
    for ch in raw.chars() {
        match ch {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    out
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

// The write and the name rules are shared with `ewo-core` (its `pvp.toml`
// writer uses both), so they live there; everything in the launcher keeps
// importing them from here.
pub use ewo_core::util::{atomic_write, validate_name, NameError};

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
    fn path_to_file_url_round_trips_through_file_url_to_path() {
        let p = Path::new(if cfg!(windows) {
            "C:\\a b\\100%\\x#1.jar"
        } else {
            "/a b/100%/x#1.jar"
        });
        let url = path_to_file_url(p);
        assert!(url.starts_with("file:///"), "{url}");
        assert!(url.ends_with("/a%20b/100%25/x%231.jar"), "{url}");
        let back = file_url_to_path(&url).unwrap();
        assert_eq!(
            back.to_string_lossy().replace('\\', "/"),
            p.to_string_lossy().replace('\\', "/")
        );
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
        assert_eq!(
            validate_name("käyttäjä's world 2", 48),
            Ok("käyttäjä's world 2")
        );
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
