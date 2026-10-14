//! Pure mappings from launcher state to the strings and rows the renderer and
//! the social heartbeat show: friend rows, screen names, auth-error text.
//! Moved out of `main.rs` unchanged.

use super::*;

/// Phase H5: map a `social::FriendEntry` (raw bot payload) to a
/// renderer-ready `FriendRowView`. The launcher doesn't yet know MC
/// display names (no Mojang UUID→name resolver), so we fall back to
/// "Player <last-8-of-uuid>" — good enough for v1, polish later.
pub(crate) fn friend_entry_to_view(entry: &social::FriendEntry) -> FriendRowView {
    let display_name = entry
        .minecraft_uuid
        .as_deref()
        .map(|u| {
            let cleaned: String = u.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if cleaned.len() >= 8 {
                format!("Player {}", &cleaned[cleaned.len() - 8..])
            } else {
                format!("Player {}", u)
            }
        })
        .unwrap_or_else(|| "Unknown".into());

    let (presence, online) = match entry.presence.as_ref() {
        None => ("offline".to_string(), false),
        Some(p) => {
            let where_at = match p.location.as_str() {
                "launcher" => p
                    .screen
                    .as_deref()
                    .map(|s| format!("in launcher · {}", s))
                    .unwrap_or_else(|| "in launcher".into()),
                "in_game" => p
                    .server_addr
                    .as_deref()
                    .map(|s| format!("in-game · {}", s))
                    .unwrap_or_else(|| "in-game".into()),
                other => other.to_string(),
            };
            (where_at, true)
        }
    };

    // H6 — joinable address: only when the friend is actually in-game.
    let server_addr = entry.presence.as_ref().and_then(|p| {
        if p.location == "in_game" {
            p.server_addr.clone()
        } else {
            None
        }
    });

    FriendRowView {
        display_name,
        presence,
        online,
        discord_id: entry.discord_id.clone(),
        server_addr,
    }
}

/// Stable lowercase name for a launcher screen — used as the `screen`
/// field on Phase H3 presence heartbeats. Friends see this verbatim
/// (e.g. "in_launcher · settings"), so keep the strings short.
pub(crate) fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::MainMenu => "main_menu",
        Screen::Instances => "instances",
        Screen::Friends => "friends",
        Screen::Settings => "settings",
        Screen::Launching => "launching",
    }
}

/// Render an `AuthError` into a user-facing string for the Account tab's
/// detail line. The XSTS variants get the spec-defined messages
/// (region-blocked etc.); other errors get a clean fallback.
///
/// `Other` errors are pattern-matched for known patterns so the user
/// sees something readable instead of raw HTTP-response JSON.
pub(crate) fn format_auth_error(err: &auth::AuthError) -> String {
    match err {
        auth::AuthError::UserCancelled => "sign-in was cancelled.".to_string(),
        auth::AuthError::Network(msg) => format!("network error: {}", msg),
        auth::AuthError::XstsBlocked(b) => b.user_message().to_string(),
        auth::AuthError::NoMinecraftLicense => {
            "this Microsoft account doesn't appear to own Minecraft Java edition.".to_string()
        }
        auth::AuthError::Other(msg) => friendly_other_error(msg),
    }
}

/// Best-effort transform of raw HTTP-response error strings into a clean
/// one-liner. Returns the raw message unchanged if no pattern matches.
pub(crate) fn friendly_other_error(msg: &str) -> String {
    if msg.contains("Invalid app registration") {
        return "this Entra app isn't yet approved by Mojang's Minecraft Launcher \
            Program. apply at aka.ms/AppRegInfo, or use Phase B (no auth) for now."
            .to_string();
    }
    if msg.contains("AADSTS") {
        // Pull just the AADSTS code + short message instead of the full body.
        if let Some(idx) = msg.find("AADSTS") {
            let tail = &msg[idx..];
            let line = tail.split('\n').next().unwrap_or(tail);
            return format!("Microsoft auth: {}", line);
        }
    }
    // Fallback: trim whitespace + cap length so the panel doesn't overflow.
    let trimmed = msg.trim();
    if trimmed.len() > 240 {
        format!("{}…", &trimmed[..240])
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::{FriendEntry, FriendPresence};

    fn entry(uuid: Option<&str>, presence: Option<FriendPresence>) -> FriendEntry {
        FriendEntry {
            discord_id: "d1".into(),
            minecraft_uuid: uuid.map(str::to_string),
            presence,
        }
    }
    fn presence(location: &str, server: Option<&str>, screen: Option<&str>) -> FriendPresence {
        FriendPresence {
            location: location.into(),
            server_addr: server.map(str::to_string),
            screen: screen.map(str::to_string),
        }
    }

    #[test]
    fn friend_name_is_last_eight_hex_digits_of_the_uuid() {
        let v = friend_entry_to_view(&entry(Some("069a79f4-44e9-4726-a5be-fca90e38aaf5"), None));
        assert_eq!(v.display_name, "Player 0e38aaf5");
        assert_eq!(v.presence, "offline");
        assert!(!v.online);
        assert_eq!(v.server_addr, None);
        assert_eq!(v.discord_id, "d1");
    }

    #[test]
    fn friend_name_falls_back_for_short_or_missing_uuids() {
        assert_eq!(
            friend_entry_to_view(&entry(Some("abc"), None)).display_name,
            "Player abc"
        );
        assert_eq!(
            friend_entry_to_view(&entry(None, None)).display_name,
            "Unknown"
        );
    }

    #[test]
    fn friend_presence_text_and_joinability() {
        let launcher = friend_entry_to_view(&entry(
            None,
            Some(presence("launcher", Some("ignored:1"), Some("settings"))),
        ));
        assert_eq!(launcher.presence, "in launcher · settings");
        assert!(launcher.online);
        assert_eq!(
            launcher.server_addr, None,
            "only in-game friends are joinable"
        );

        let bare = friend_entry_to_view(&entry(None, Some(presence("launcher", None, None))));
        assert_eq!(bare.presence, "in launcher");

        let in_game = friend_entry_to_view(&entry(
            None,
            Some(presence("in_game", Some("play.example:25565"), None)),
        ));
        assert_eq!(in_game.presence, "in-game · play.example:25565");
        assert_eq!(in_game.server_addr.as_deref(), Some("play.example:25565"));

        let no_addr = friend_entry_to_view(&entry(None, Some(presence("in_game", None, None))));
        assert_eq!(no_addr.presence, "in-game");
        assert_eq!(no_addr.server_addr, None);

        let other = friend_entry_to_view(&entry(None, Some(presence("afk", None, None))));
        assert_eq!(other.presence, "afk");
        assert!(other.online);
    }

    #[test]
    fn screen_names_are_the_stable_heartbeat_strings() {
        assert_eq!(screen_name(Screen::MainMenu), "main_menu");
        assert_eq!(screen_name(Screen::Instances), "instances");
        assert_eq!(screen_name(Screen::Friends), "friends");
        assert_eq!(screen_name(Screen::Settings), "settings");
        assert_eq!(screen_name(Screen::Launching), "launching");
    }

    #[test]
    fn auth_errors_render_user_facing_text() {
        assert_eq!(
            format_auth_error(&auth::AuthError::UserCancelled),
            "sign-in was cancelled."
        );
        assert_eq!(
            format_auth_error(&auth::AuthError::Network("timeout".into())),
            "network error: timeout"
        );
        assert_eq!(
            format_auth_error(&auth::AuthError::XstsBlocked(
                auth::XstsBlock::RegionBlocked
            )),
            auth::XstsBlock::RegionBlocked.user_message()
        );
        assert!(format_auth_error(&auth::AuthError::NoMinecraftLicense).contains("own Minecraft"));
    }

    #[test]
    fn other_auth_errors_are_made_readable() {
        assert!(friendly_other_error("... Invalid app registration ...").contains("AppRegInfo"));
        assert_eq!(
            friendly_other_error("{\"error\":\"x\"} AADSTS70000: bad grant\nmore detail"),
            "Microsoft auth: AADSTS70000: bad grant"
        );
        assert_eq!(friendly_other_error("  plain  "), "plain");
        let long = "x".repeat(300);
        assert_eq!(friendly_other_error(&long), format!("{}…", "x".repeat(240)));
    }
}
