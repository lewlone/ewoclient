//! `Util.OS.openUri` — handing a clicked chat link to the desktop (M128).
//!
//! ```java
//! public void openUri(final URI uri) {
//!    try {
//!       Process process = Runtime.getRuntime().exec(this.getOpenUriArguments(uri));
//!       process.getInputStream().close();
//!       process.getErrorStream().close();
//!       process.getOutputStream().close();
//!    } catch (IOException e) {
//!       Util.LOGGER.error("Couldn't open location '{}'", uri, e);
//!    }
//! }
//! ```
//!
//! `getOpenUriArguments` is the enum's one per-platform override:
//!
//! | `Util.OS` | argv |
//! |---|---|
//! | `WINDOWS` | `rundll32`, `url.dll,FileProtocolHandler`, uri |
//! | `OSX` | `open`, uri |
//! | everything else | `xdg-open`, uri |
//!
//! **`Runtime.exec(String[])` is not a shell**, and neither is
//! [`std::process::Command`] — the URI is one argument, never a command line,
//! so there is nothing to quote and nothing to inject into.
//!
//! # The gate is applied twice on purpose
//!
//! [`rewo_world::chat_events::parse_untrusted_uri`] refuses anything but
//! `http`/`https` at **decode**, which is where vanilla refuses it too
//! (`ExtraCodecs.UNTRUSTED_URI` is `OpenUrl`'s field codec), so a `file:` or
//! `javascript:` link never becomes a `ClickEvent::OpenUrl` at all. This
//! re-checks anyway, because the cost is one string comparison and the failure
//! mode of not having it is launching whatever a server names.
//!
//! Vanilla's `Screen.clickUrlAction` adds two option gates on top —
//! `chatLinks()` (default on) refuses outright, and `chatLinksPrompt()`
//! (default **on**) puts a `ConfirmLinkScreen` in front. Both live outside this
//! file: [`rewo_ui::confirm_link_screen`] owns the screen and the decision
//! ([`rewo_ui::confirm_link_screen::click_url_action`]), `rewo_net::options`
//! reads the two booleans from `options.txt`, and the live client only calls
//! [`open_uri`] once the player has said yes (or has turned the prompt off).
//!
//! # The child does not inherit the session's secrets
//!
//! The opener is a child of the client, and the launcher gives the client
//! `REWO_ACCESS_TOKEN`. `Runtime.exec` in vanilla inherits the environment too,
//! but vanilla's environment holds no Minecraft token; Rewo's does, and a
//! browser launcher (or whatever a `xdg-open` handler chain ends up running)
//! has no business with it. [`scrub_env`] removes every `REWO_*` variable and
//! anything whose name says it is a token, secret or password.

/// `Util.OS.getOpenUriArguments` for the platform this binary was built for.
///
/// Split out from [`open_uri`] because it is the whole security-relevant part
/// and the only part a test can see: the process launch itself is a side
/// effect on the user's desktop.
pub fn open_uri_args(uri: &str) -> Vec<String> {
    let uri = uri.to_owned();
    if cfg!(target_os = "windows") {
        vec!["rundll32".into(), "url.dll,FileProtocolHandler".into(), uri]
    } else if cfg!(target_os = "macos") {
        vec!["open".into(), uri]
    } else {
        vec!["xdg-open".into(), uri]
    }
}

/// Whether an environment variable must not reach the opener child.
///
/// `REWO_*` is every knob and credential the launcher hands the client
/// (`REWO_ACCESS_TOKEN` among them). The substring rule catches the rest of the
/// shapes a session accumulates (`GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY`,
/// `DB_PASSWORD`), case-insensitively because Windows folds variable names.
pub fn is_secret_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("REWO_")
        || upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
}

/// Remove every variable [`is_secret_env`] names from `cmd`'s environment —
/// the inherited ones and any set explicitly on it.
pub fn scrub_env(cmd: &mut std::process::Command) {
    let explicit: Vec<std::ffi::OsString> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
    let doomed: Vec<std::ffi::OsString> = std::env::vars_os()
        .map(|(k, _)| k)
        .chain(explicit)
        .filter(|k| k.to_str().is_some_and(is_secret_env))
        .collect();
    for k in doomed {
        cmd.env_remove(k);
    }
}

/// The opener process for `uri`, environment scrubbed, not yet spawned.
fn opener_command(uri: &str) -> std::process::Command {
    let args = open_uri_args(uri);
    let (program, rest) = args.split_first().expect("argv is never empty");
    let mut cmd = std::process::Command::new(program);
    cmd.args(rest);
    scrub_env(&mut cmd);
    cmd
}

/// Open a clicked chat link, or refuse it.
///
/// Returns whether the launch was attempted, so a caller can log the refusal
/// rather than guess at silence.
pub fn open_uri(uri: &str) -> bool {
    // Defence in depth: the decode already guarantees this (see the module
    // docs), so reaching the `else` means something upstream changed.
    if rewo_world::chat_events::parse_untrusted_uri(uri).is_none() {
        log::warn!("chat: refusing to open a non-http(s) uri");
        return false;
    }
    match opener_command(uri).spawn() {
        Ok(_) => {
            log::info!("chat: opened a link");
            true
        }
        Err(e) => {
            // `catch (IOException e) { LOGGER.error(...) }` — a failed launch
            // is logged, not propagated.
            log::error!("chat: couldn't open location: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URI is one argv element, so there is no command line to escape and
    /// a hostile one cannot become a second command.
    #[test]
    fn the_uri_is_a_single_argument() {
        let args = open_uri_args("https://example.com/a?b=c&d");
        assert_eq!(args.last().unwrap(), "https://example.com/a?b=c&d");
        assert_eq!(args.len(), if cfg!(target_os = "windows") { 3 } else { 2 });
    }

    /// `Util.OS.WINDOWS.getOpenUriArguments` is
    /// `{"rundll32", "url.dll,FileProtocolHandler", uri}` — the middle element
    /// is one argument with a comma in it, not two.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_windows_argv_is_vanillas() {
        assert_eq!(
            open_uri_args("http://x"),
            ["rundll32", "url.dll,FileProtocolHandler", "http://x"]
        );
    }

    #[test]
    fn secret_looking_names_are_recognised() {
        for n in [
            "REWO_ACCESS_TOKEN",
            "rewo_game_dir",
            "GITHUB_TOKEN",
            "My_Api_Secret",
            "DB_PASSWORD",
            "ACCESS_TOKEN_FILE",
        ] {
            assert!(is_secret_env(n), "{n}");
        }
        for n in [
            "PATH",
            "HOME",
            "APPDATA",
            "DISPLAY",
            "XDG_RUNTIME_DIR",
            "REWORK",
        ] {
            assert!(!is_secret_env(n), "{n}");
        }
    }

    /// **The child's environment, not a list of names.** An explicit variable
    /// stands in for an inherited one (setting the test process's own
    /// environment would race the other tests), and the child prints what it
    /// received. `REWO_ACCESS_TOKEN` and a token-named variable must be gone
    /// and an unrelated one must survive — the last assertion is what keeps a
    /// scrub that clears everything from passing.
    ///
    /// MUTATION: `scrub_env` as a no-op, or `is_secret_env` without the
    /// `REWO_` prefix arm.
    #[test]
    fn the_opener_child_does_not_receive_the_session_token() {
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "set"]);
            c
        } else {
            std::process::Command::new("env")
        };
        cmd.env("REWO_ACCESS_TOKEN", "s3cr3t-session-value")
            .env("SOME_OTHER_TOKEN", "t0ken-value")
            .env("REWO_GAME_DIR", "/tmp/rewo-dir-value")
            .env("HARMLESS_VAR", "harmless-value");
        scrub_env(&mut cmd);
        let out = cmd.output().expect("the environment dumper runs");
        let text = String::from_utf8_lossy(&out.stdout);
        for leaked in ["s3cr3t-session-value", "t0ken-value", "rewo-dir-value"] {
            assert!(!text.contains(leaked), "{leaked} reached the child");
        }
        assert!(
            text.contains("harmless-value"),
            "an unrelated variable survives:\n{text}"
        );
    }

    /// The real opener goes through the same scrub.
    #[test]
    fn the_opener_command_is_scrubbed() {
        let cmd = opener_command("https://example.com");
        // `get_envs` reports a removal as `(name, None)`.
        let removed: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .filter_map(|(k, _)| k.to_str().map(str::to_owned))
            .collect();
        for k in &removed {
            assert!(is_secret_env(k), "{k} was removed but is not secret");
        }
        // Whatever secret-shaped names this process really has are in the list.
        for (k, _) in std::env::vars_os() {
            if let Some(k) = k.to_str().filter(|k| is_secret_env(k)) {
                assert!(removed.iter().any(|r| r == k), "{k} would be inherited");
            }
        }
    }

    /// The second gate. Nothing that reaches here should fail it, and it
    /// fails closed if something does.
    #[test]
    fn a_non_http_uri_is_refused_without_launching_anything() {
        assert!(!open_uri("file:///C:/windows/system32/calc.exe"));
        assert!(!open_uri("javascript:alert(1)"));
        assert!(!open_uri("steam://run/1"));
        assert!(!open_uri(""));
    }
}
