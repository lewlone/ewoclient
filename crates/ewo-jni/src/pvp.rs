//! PvP-Utils config — re-export of [`ewo_core::pvp`].
//!
//! The config lives in `ewo-core` so the launcher's Settings → PvP-Utils tab
//! and the in-game overlay editor share one source of truth. This shim keeps
//! `crate::pvp::*` imports inside `hud.rs` working unchanged.

pub use ewo_core::pvp::*;
