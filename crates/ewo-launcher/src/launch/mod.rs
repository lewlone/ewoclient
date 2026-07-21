//! v2 phase C — JVM spawn + game launch.
//!
//! Given a Ready instance, build a `LaunchPlan` (substitute template
//! tokens, resolve classpath, pick natives), extract the natives JAR
//! contents into the per-instance `natives/` dir, and spawn the JVM
//! with the resolved args. Stream stdout/stderr back to the UI.

pub mod jre;
pub mod natives;
pub mod plan;
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

/// Argv for launching Rewo as the real playable client against a server —
/// `rewo live` (NOT `view`, the M2 static snapshot; spawning `view` here
/// was the launcher-integration gap flagged in REWO_PLAN §0.0). Username
/// and token ride the `REWO_*` env contract, never argv (§9.1).
pub fn native_client_args(host: &str, port: u16, version: &str) -> Vec<String> {
    vec![
        "live".into(),
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
        "--version".into(),
        version.into(),
    ]
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
        let args = native_client_args("play.example.net", 25565, "26.2");
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
}
