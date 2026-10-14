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

/// How far past the expected size a download body may run before it is cut
/// off (a CDN adding a trailing newline is fine; a stream that never ends
/// is not).
pub const BODY_SLACK_BYTES: u64 = 64 * 1024;

/// The most an HTTP body of *unknown* expected size may be (a manifest says
/// `size: 0`). Larger than any single Mojang asset, library or client jar.
pub const UNKNOWN_SIZE_BODY_CAP: u64 = 1 << 30;

/// The most bytes to accept for a body whose expected size is `expected`
/// (`0` = unknown).
pub fn body_limit(expected: u64) -> u64 {
    if expected == 0 {
        UNKNOWN_SIZE_BODY_CAP
    } else {
        expected.saturating_add(BODY_SLACK_BYTES)
    }
}

/// Fail early when the server's declared `Content-Length` is already past
/// `limit`; an absent or unparseable header says nothing, the streaming cap
/// in [`copy_capped`] still applies.
pub fn check_declared_length(content_length: Option<&str>, limit: u64) -> Result<(), String> {
    match content_length.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(n) if n > limit => Err(format!(
            "server announced {n} bytes, over the {limit}-byte limit"
        )),
        _ => Ok(()),
    }
}

/// Stream `reader` into `sink` in chunks, failing as soon as more than
/// `limit` bytes have arrived (at most one chunk past it is read). Returns
/// the byte count. The sink's error aborts the copy.
pub fn copy_capped(
    reader: &mut dyn io::Read,
    limit: u64,
    mut sink: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        if total > limit {
            return Err(format!("body is larger than the {limit}-byte limit"));
        }
        sink(&buf[..n])?;
    }
}

/// [`atomic_write`] for a file holding credentials: on Unix the file is
/// created mode 0600 (never briefly world-readable). On Windows the per-user
/// `%APPDATA%` ACL already applies. The implementation lives in `ewo-core`.
pub use ewo_core::util::atomic_write_private;

/// Preserve a config file that failed to parse by copying it to
/// `<name>.bak-<unix_ts>` before any save can overwrite it. Logs an error
/// either way; returns the backup path on success.
pub fn backup_unparseable(path: &Path, reason: &dyn std::fmt::Display) -> Option<PathBuf> {
    backup_unparseable_with(path, reason, false)
}

/// What reading a config file at startup found.
#[derive(Debug)]
pub enum ConfigRead {
    /// No such file: a fresh install, defaults are right.
    Missing,
    Text(String),
    /// The file exists but is not valid UTF-8: treat it like a parse error
    /// (back it up, then use defaults).
    Corrupt(String),
    /// The file exists but could not be read (locked by a scanner, ACL, I/O
    /// error). Its content is unknown and may be perfectly good.
    Unreadable,
}

/// Stops saves from overwriting a config file the launcher failed to *read*.
///
/// A read error is not a parse error: the file may hold every account or
/// instance the user has. The in-memory state falls back to defaults, but if
/// the next save then wrote those defaults over the file, a transient
/// failure would destroy the data. Once a guard is blocked its owner's saves
/// are skipped (loudly) for the rest of the session.
pub struct SaveGuard(std::sync::atomic::AtomicBool);

impl SaveGuard {
    pub const fn new() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }

    pub fn block(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_blocked(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether a save of `path` may proceed; logs an error when not.
    pub fn allows_save(&self, path: &Path) -> bool {
        if self.is_blocked() {
            log::error!(
                "NOT saving {}: it could not be read at startup, and saving defaults over it \
                 would destroy it. Fix the file's permissions or restore it, then restart.",
                path.display()
            );
            return false;
        }
        true
    }

    /// Back up an unparseable file; if even the backup fails, block saves so
    /// the original is not overwritten.
    pub fn backup_or_block(&self, path: &Path, reason: &dyn std::fmt::Display, private: bool) {
        if backup_unparseable_with(path, reason, private).is_none() {
            self.block();
        }
    }
}

impl Default for SaveGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Read a config file, blocking `guard` on an error that isn't "not found".
pub fn read_config(path: &Path, guard: &SaveGuard) -> ConfigRead {
    match fs::read_to_string(path) {
        Ok(s) => ConfigRead::Text(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => ConfigRead::Missing,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => ConfigRead::Corrupt(e.to_string()),
        Err(e) => {
            log::error!(
                "could not read {}: {e} — starting with defaults but NOT overwriting the file",
                path.display()
            );
            guard.block();
            ConfigRead::Unreadable
        }
    }
}

/// The copy of a file that may hold credentials (`private`) gets the same
/// owner-only permissions as the original; a plain `fs::copy` of a plaintext
/// `auth.toml` would make a looser duplicate.
fn backup_unparseable_with(
    path: &Path,
    reason: &dyn std::fmt::Display,
    private: bool,
) -> Option<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".bak-{ts}"));
    let backup = path.with_file_name(name);
    let copied = if private {
        fs::read(path).and_then(|bytes| atomic_write_private(&backup, &bytes))
    } else {
        fs::copy(path, &backup).map(|_| ())
    };
    match copied {
        Ok(()) => {
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

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ewo-util-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_is_a_fresh_install_not_a_read_error() {
        let dir = scratch("cfg-missing");
        let guard = SaveGuard::new();
        let p = dir.join("auth.toml");
        assert!(matches!(read_config(&p, &guard), ConfigRead::Missing));
        assert!(!guard.is_blocked());
        assert!(guard.allows_save(&p));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_error_blocks_saves_but_a_parse_class_failure_does_not() {
        let dir = scratch("cfg-readerr");
        // A directory where the file should be: `read` fails with something
        // that is neither NotFound nor InvalidData, on every platform.
        let as_dir = dir.join("instances.toml");
        fs::create_dir_all(&as_dir).unwrap();
        let guard = SaveGuard::new();
        assert!(matches!(
            read_config(&as_dir, &guard),
            ConfigRead::Unreadable
        ));
        assert!(guard.is_blocked(), "an unreadable file must block saves");
        assert!(!guard.allows_save(&as_dir));

        // Invalid UTF-8 is corruption: handled by backup + defaults.
        let bad = dir.join("bad.toml");
        fs::write(&bad, [0xFF, 0xFE, 0x00, 0xFF]).unwrap();
        let guard = SaveGuard::new();
        assert!(matches!(read_config(&bad, &guard), ConfigRead::Corrupt(_)));
        assert!(!guard.is_blocked());

        // Good text reads fine and leaves saves enabled.
        let ok = dir.join("ok.toml");
        fs::write(&ok, "a = 1").unwrap();
        assert!(matches!(read_config(&ok, &guard), ConfigRead::Text(t) if t == "a = 1"));
        assert!(guard.allows_save(&ok));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failing_to_back_up_blocks_saves() {
        let dir = scratch("cfg-nobackup");
        let guard = SaveGuard::new();
        // Nothing to copy: the backup fails, so the (absent/unknown) original
        // must not be overwritten either.
        guard.backup_or_block(&dir.join("gone.toml"), &"test", false);
        assert!(guard.is_blocked());
        let ok = dir.join("auth.toml");
        fs::write(&ok, "x = 1").unwrap();
        let guard = SaveGuard::new();
        guard.backup_or_block(&ok, &"test", true);
        assert!(!guard.is_blocked());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_private_backup_has_the_same_contents() {
        let dir = scratch("cfg-private-bak");
        let p = dir.join("auth.toml");
        fs::write(&p, "ms_refresh_token = \"t\"").unwrap();
        let bak = backup_unparseable_with(&p, &"test", true).unwrap();
        assert_eq!(
            fs::read_to_string(&bak).unwrap(),
            "ms_refresh_token = \"t\""
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&bak).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the backup must not be looser than auth.toml");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_limit_is_expected_plus_slack_or_the_unknown_cap() {
        assert_eq!(body_limit(1000), 1000 + BODY_SLACK_BYTES);
        assert_eq!(body_limit(0), UNKNOWN_SIZE_BODY_CAP);
        assert_eq!(body_limit(u64::MAX), u64::MAX);
    }

    #[test]
    fn declared_length_is_checked_when_present() {
        assert!(check_declared_length(Some("100"), 100).is_ok());
        assert!(check_declared_length(Some(" 101 "), 100).is_err());
        assert!(check_declared_length(Some("chunked?"), 100).is_ok());
        assert!(check_declared_length(None, 100).is_ok());
    }

    /// A reader that never ends: the cap, not the source, stops the copy.
    struct Endless(u64);
    impl io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0 += buf.len() as u64;
            buf.fill(7);
            Ok(buf.len())
        }
    }

    #[test]
    fn copy_capped_stops_an_endless_body_near_the_limit() {
        let mut src = Endless(0);
        let mut seen = 0u64;
        let err = copy_capped(&mut src, 200_000, |c| {
            seen += c.len() as u64;
            Ok(())
        })
        .unwrap_err();
        assert!(err.contains("larger than"), "{err}");
        assert!(seen <= 200_000, "nothing past the limit reaches the sink");
        assert!(src.0 <= 200_000 + 64 * 1024, "read at most one chunk past");
    }

    #[test]
    fn copy_capped_passes_a_body_within_the_limit_and_sink_errors_abort() {
        let data = vec![1u8; 100_000];
        let mut out = Vec::new();
        let n = copy_capped(&mut &data[..], 100_000, |c| {
            out.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert_eq!((n, out.len()), (100_000, 100_000));
        let err = copy_capped(&mut &data[..], 1 << 20, |_| Err("disk full".into())).unwrap_err();
        assert_eq!(err, "disk full");
        // An exact-fit body is accepted; one byte more is not.
        assert!(copy_capped(&mut &data[..], 99_999, |_| Ok(())).is_err());
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
