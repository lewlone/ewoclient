//! Test and debug knobs (`REWO_FORCE_*`, `REWO_PRECMD`, `REWO_CLICK`, ...).
//!
//! These change client behaviour — some send server commands or synthesize
//! input — so they are honoured only in builds with the `gates` feature. A
//! default build answers "not set" for every one of them, whatever the
//! environment says, so a variable inherited from a parent process cannot
//! drive a user's client.
//!
//! Runtime configuration the launcher passes (`REWO_ACCESS_TOKEN`,
//! `REWO_USERNAME`, `REWO_SERVER`, ...) is read with `std::env` directly and
//! does not go through here.

use std::env::VarError;
use std::ffi::OsString;

/// Drop-in for `std::env::var` for a test knob.
pub fn var(name: &str) -> Result<String, VarError> {
    if cfg!(feature = "gates") {
        std::env::var(name)
    } else {
        Err(VarError::NotPresent)
    }
}

/// Drop-in for `std::env::var_os` for a test knob.
pub fn var_os(name: &str) -> Option<OsString> {
    if cfg!(feature = "gates") {
        std::env::var_os(name)
    } else {
        None
    }
}
