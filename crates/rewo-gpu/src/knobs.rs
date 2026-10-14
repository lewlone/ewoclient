//! Test and debug knobs (`REWO_FOG`, `REWO_MOB_DEBUG_TEX`, `REWO_CEM_NOANIM`,
//! `REWO_NO_SYNC_VALIDATION`).
//!
//! They change what the renderer draws or which validation runs, so they are
//! honoured only in builds with the `gates` feature (rewo-app's `gates`
//! enables it). A default build answers "not set" for every one of them,
//! whatever the environment says, so a variable inherited from a parent
//! process cannot alter a user's client. Mirrors `rewo-app/src/knobs.rs`.

use std::env::VarError;
use std::ffi::OsString;

/// Drop-in for `std::env::var` for a test knob.
pub(crate) fn var(name: &str) -> Result<String, VarError> {
    if cfg!(feature = "gates") {
        std::env::var(name)
    } else {
        Err(VarError::NotPresent)
    }
}

/// Drop-in for `std::env::var_os` for a test knob.
pub(crate) fn var_os(name: &str) -> Option<OsString> {
    if cfg!(feature = "gates") {
        std::env::var_os(name)
    } else {
        None
    }
}
