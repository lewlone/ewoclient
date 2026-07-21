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
pub use spawn::{spawn as spawn_jvm, LaunchEvent, Severity};

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
