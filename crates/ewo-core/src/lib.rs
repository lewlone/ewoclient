//! `ewo-core` — pure logic. State, types, theme tokens, easing, animation engine.
//!
//! Zero rendering dependencies on purpose. This crate compiles in seconds and is
//! the safe place to iterate on app behavior without paying Skia compile times.
//!
//! See `CLAUDE.md` for the full architecture rationale and non-negotiables.

pub mod color;
pub mod easing;
pub mod modules;
pub mod state;
pub mod theme;

pub use color::{OkLch, Srgb, Srgba};
pub use easing::CubicBezier;
pub use state::Screen;
pub use theme::{Settings, Theme};
