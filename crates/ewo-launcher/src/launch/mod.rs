//! v2 phase C — JVM spawn + game launch.
//!
//! Given a Ready instance, build a `LaunchPlan` (substitute template
//! tokens, resolve classpath, pick natives), extract the natives JAR
//! contents into the per-instance `natives/` dir, and spawn the JVM
//! with the resolved args. Stream stdout/stderr back to the UI.

pub mod jre;
pub mod natives;
pub mod plan;
pub mod spawn;

pub use jre::{detect_all as detect_jres, pick_for_major as pick_jre, DetectedJre};
pub use natives::{extract_all, ExtractError};
pub use plan::{build, BuildError, LaunchPlan, LaunchProfile};
pub use spawn::{spawn as spawn_jvm, LaunchEvent, Severity};
