//! In-game FRIENDS overlay tab data — reads the `ewo-friends.txt` snapshot
//! the launcher writes into the active profile dir (Phase H).
//!
//! The lean cdylib makes **no** network calls: the launcher owns all social
//! HTTP and drops a fresh snapshot whenever the friends list changes. The
//! overlay tab reads the small file directly each frame it's visible — no
//! cache needed (the file is a few hundred bytes and the tab only renders
//! while the overlay is open).
//!
//! Format: one tab-separated line per accepted friend —
//! `<online 0|1>\t<name>\t<presence>\t<server_addr>`.
//!
//! Freshness caveat: the launcher only rewrites the snapshot while it's
//! running its per-frame loop (i.e. while it's the foreground window). During
//! active play the launcher is backgrounded, so the list reflects the most
//! recent launcher-foreground refresh (typically launch time). Live updates
//! during play would need either an in-game poller or a launcher background
//! tick — deferred.

use std::path::PathBuf;

/// One friend row, decoded from a line of `ewo-friends.txt`.
pub struct FriendLine {
    pub online: bool,
    pub name: String,
    pub presence: String,
    /// The joinable address when the friend is in-game, else empty.
    pub server_addr: String,
}

/// `%APPDATA%/EwoClient/profiles/<active>/ewo-friends.txt`. Mirrors the path
/// resolution in [`crate::crosshair::crosshair_toml_path`].
fn friends_file_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let profile = crate::hud::read_active_profile().unwrap_or_else(|| "Default".to_string());
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(profile)
            .join("ewo-friends.txt"),
    )
}

/// Read + parse the current snapshot. Returns an empty `Vec` when the file is
/// absent (launcher signed out / not linked) — the tab renders its empty
/// state. Cheap enough to call per-frame while the overlay is open.
pub fn read_friends() -> Vec<FriendLine> {
    let Some(path) = friends_file_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let online = parts.next() == Some("1");
        let name = parts.next().unwrap_or("").to_string();
        let presence = parts.next().unwrap_or("").to_string();
        let server_addr = parts.next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        out.push(FriendLine {
            online,
            name,
            presence,
            server_addr,
        });
    }
    out
}
