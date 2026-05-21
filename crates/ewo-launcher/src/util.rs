//! Small launcher-wide helpers that don't belong to any one subsystem.

use std::path::PathBuf;

/// Convert a `file://` URL into a filesystem path, or return `None` for a
/// non-`file://` URL.
///
/// Handles both unix-style `file:///home/user/x.jar` → `/home/user/x.jar`
/// and Windows-style `file:///C:/path/to.jar` → `C:/path/to.jar` (the
/// leading `/` before the drive letter is stripped). Includes a naive
/// percent-decoder so paths with `%20` etc. round-trip; authority and
/// query strings aren't handled — loader/library URLs don't use them.
pub fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Strip the leading `/` on Windows-style `file:///C:/...` so the
    // result becomes `C:/...` (a real drive-letter path), not `/C:/...`.
    let trimmed = if cfg!(windows)
        && rest.starts_with('/')
        && rest.len() >= 4
        && rest.as_bytes()[2] == b':'
    {
        &rest[1..]
    } else {
        rest
    };
    let mut decoded = String::with_capacity(trimmed.len());
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                decoded.push(((h << 4) | l) as u8 as char);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    Some(PathBuf::from(decoded))
}
