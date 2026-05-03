//! `ewo-render` — Skia wrapper, text shaping, particles, fx, and primitives.
//!
//! This crate owns the GPU. It produces frames. It does not know about widgets
//! or screens — that's `ewo-ui`.
//!
//! See `CLAUDE.md` for the render-graph specification (one frame, top to
//! bottom) and the build-sequence step that introduces each subsystem.

pub mod app_window;
pub mod backdrop;
pub mod frame;
pub mod gl_backend;
pub mod screens;
pub mod text;
pub mod widgets;

pub use frame::Clock;
pub use gl_backend::GlBackend;
pub use skia_safe;
pub use text::FontStore;
pub use widgets::VbtnState;
