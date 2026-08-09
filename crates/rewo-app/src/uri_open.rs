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
//! (default **on**) puts a `ConfirmLinkScreen` in front. Rewo has no options
//! file and no confirm screen, so it takes the `chatLinks` default and **skips
//! the prompt**, which is a real divergence in the user's favour only if you
//! think a confirmation is noise. It is named here rather than silently
//! absent; the prompt is a screen, and a screen is its own milestone.

/// `Util.OS.getOpenUriArguments` for the platform this binary was built for.
///
/// Split out from [`open_uri`] because it is the whole security-relevant part
/// and the only part a test can see: the process launch itself is a side
/// effect on the user's desktop.
pub fn open_uri_args(uri: &str) -> Vec<String> {
    let uri = uri.to_owned();
    if cfg!(target_os = "windows") {
        vec![
            "rundll32".into(),
            "url.dll,FileProtocolHandler".into(),
            uri,
        ]
    } else if cfg!(target_os = "macos") {
        vec!["open".into(), uri]
    } else {
        vec!["xdg-open".into(), uri]
    }
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
    let args = open_uri_args(uri);
    let (program, rest) = args.split_first().expect("argv is never empty");
    match std::process::Command::new(program).args(rest).spawn() {
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
