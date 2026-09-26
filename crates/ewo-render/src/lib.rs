//! `ewo-render` — Skia wrapper, text shaping, particles, fx, and primitives.
//!
//! This crate owns the GPU and everything drawn with it: the backdrop, the
//! widgets (`widgets`), and the launcher screens (`screens`).

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
