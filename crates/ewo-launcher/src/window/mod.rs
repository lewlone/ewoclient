//! Per-platform custom-frame window setup.
//!
//! The launcher renders its entire chrome (rounded corners, drop shadow, hairline
//! rim) itself. We strip native decorations and route hit-testing for drag and
//! resize through OS-specific APIs:
//!
//! - **Windows:** `WM_NCHITTEST` (zone reporting), `WM_NCCALCSIZE` (strip non-
//!   client area), `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, ROUND)`
//!   for free Win11 corner rounding + DWM shadow.
//! - **Linux/Hyprland:** `xdg-decoration` mode `client-side`, `xdg_toplevel.move()`
//!   / `resize()` requests for drag and edge-resize. Hyprland (wlroots) honors
//!   these cleanly.
//!
//! No X11 implementation. CachyOS + Hyprland is the only Linux target.
//!
//! See `CLAUDE.md` build-sequence step 1 for the implementation checklist.

use winit::window::Window;

#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "linux")]
mod wayland;

/// Apply platform-specific custom-frame setup to a freshly created window.
/// Called once after `Window::create_window` returns.
pub fn configure(window: &Window) {
    #[cfg(target_os = "windows")]
    win32::configure(window);

    #[cfg(target_os = "linux")]
    wayland::configure(window);
}
