//! v2 phase C — JVM spawn + game launch.
//!
//! Given a Ready instance, build a `LaunchPlan` (substitute template
//! tokens, resolve classpath, pick natives), extract the natives JAR
//! contents into the per-instance `natives/` dir, and spawn the JVM
//! with the resolved args. Stream stdout/stderr back to the UI.

pub mod jre;
pub mod natives;
pub mod plan;
pub mod prepare;
pub mod reaper;
pub mod spawn;

pub use jre::{detect_all as detect_jres, pick_for_major as pick_jre};
pub use natives::extract_all;
pub use plan::{build, LaunchProfile};
pub use spawn::{spawn as spawn_jvm, spawn_native, LaunchEvent, NativePlan, Severity};

/// Locate the Rewo binary: same directory as the launcher exe — covers both
/// the shared cargo target dir (dev) and the dist bundle (release).
pub fn find_rewo_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "rewo.exe" } else { "rewo" };
    let candidate = dir.join(name);
    candidate.exists().then_some(candidate)
}

/// The in-game HUD native (`ewo_jni.dll`) shipped next to the launcher exe,
/// if any — `package.ps1` puts it in the dist bundle.
pub fn find_hud_native() -> Option<std::path::PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("ewo_jni.dll");
    candidate.is_file().then_some(candidate)
}

/// Argv for launching Rewo as the real playable client against a server —
/// `rewo live` (NOT `view`, the M2 static snapshot; spawning `view` here
/// was the launcher-integration gap flagged in REWO_PLAN §0.0). Username
/// and token ride the `REWO_*` env contract, never argv (§9.1).
///
/// `port` is `None` when the player typed no port: `--port` is then left off
/// so the client resolves `_minecraft._tcp.<host>` SRV and falls back to 25565,
/// as vanilla does. A port that was typed is always passed (and never
/// redirected).
pub fn native_client_args(host: &str, port: Option<u16>, version: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["live".into(), "--host".into(), host.into()];
    if let Some(port) = port {
        args.push("--port".into());
        args.push(port.to_string());
    }
    args.push("--version".into());
    args.push(version.into());
    args
}

/// Split a server address the player typed (`host`, `host:port`, `[v6]:port`)
/// into what [`native_client_args`] takes.
///
/// No port stays `None` — that is the SRV-eligible case. A bracketed or bare
/// IPv6 literal is handed through whole (the client parses it). A port that is
/// not a number keeps the launcher's old reading, 25565 written out.
pub fn split_server_address(addr: &str) -> (String, Option<u16>) {
    let addr = addr.trim();
    if addr.starts_with('[') || addr.matches(':').count() != 1 {
        return (addr.to_string(), None);
    }
    match addr.split_once(':') {
        Some((h, "")) => (h.to_string(), None),
        Some((h, p)) => (h.to_string(), Some(p.parse::<u16>().unwrap_or(25565))),
        None => (addr.to_string(), None),
    }
}

/// Suppress the console window a child console program (`java.exe`,
/// `where`, …) would otherwise pop when spawned from a GUI process.
///
/// On Windows, spawning a console subsystem executable from a process
/// with no console of its own allocates a fresh console window per child
/// — that's the "dozen cmd windows flashing on launch" the JRE scan
/// produced. `CREATE_NO_WINDOW` (0x0800_0000) suppresses that window
/// while leaving redirected stdio pipes fully intact, so we still capture
/// `-version` output and the game's stdout/stderr. No-op off Windows.
pub(crate) fn no_window(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Launching a Native instance must open the real client (`live`), not
    /// the M2 snapshot viewer — the regression REWO_PLAN §0.0 flagged.
    #[test]
    fn native_launch_uses_the_live_client() {
        let args = native_client_args("play.example.net", Some(25565), "26.2");
        assert_eq!(args[0], "live");
        assert_eq!(
            args[1..],
            [
                "--host",
                "play.example.net",
                "--port",
                "25565",
                "--version",
                "26.2"
            ]
            .map(String::from)
        );
    }

    /// An address typed without a port reaches the client without `--port`,
    /// which is what lets it follow an SRV record. Passing 25565 here (the old
    /// behaviour) would make every launcher join skip the lookup.
    #[test]
    fn a_portless_address_passes_no_port_so_the_client_can_follow_srv() {
        let args = native_client_args("play.example.net", None, "26.2");
        assert!(!args.iter().any(|a| a == "--port"), "{args:?}");
        assert_eq!(
            args,
            ["live", "--host", "play.example.net", "--version", "26.2"].map(String::from)
        );
    }

    #[test]
    fn a_typed_address_is_split_without_inventing_a_port() {
        let s = |a: &str| split_server_address(a);
        assert_eq!(s("play.example.net"), ("play.example.net".into(), None));
        assert_eq!(
            s("play.example.net:25566"),
            ("play.example.net".into(), Some(25566))
        );
        assert_eq!(
            s("play.example.net:25565"),
            ("play.example.net".into(), Some(25565))
        );
        assert_eq!(s("  play.example.net  "), ("play.example.net".into(), None));
        assert_eq!(s("play.example.net:"), ("play.example.net".into(), None));
        assert_eq!(
            s("play.example.net:abc"),
            ("play.example.net".into(), Some(25565))
        );
        assert_eq!(
            s("[::1]:25570"),
            ("[::1]:25570".into(), None),
            "the client parses brackets"
        );
        assert_eq!(s("::1"), ("::1".into(), None));
    }
}
