//! Per-platform custom-frame window setup.
//!
//! The launcher renders its entire chrome (rounded corners, drop shadow, hairline
//! rim) itself. We strip native decorations and route hit-testing for drag and
//! resize through OS-specific APIs:
//!
//! - **Windows:** `WM_NCHITTEST` (zone reporting), `WM_NCCALCSIZE` (strip non-
//!   client area), `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE,
//!   DWMWCP_DONOTROUND)` so DWM does NOT round over the 22px corners we paint
//!   ourselves (its ~8px rounding would clip ours and add a competing
//!   rectangular shadow).
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

/// LEAK_HUNT_INSTRUMENT — strip before release.
/// Current process memory snapshot — `(working_set_bytes, private_bytes)`.
/// Returns `None` on platforms where this isn't wired up.
pub fn process_memory() -> Option<(u64, u64)> {
    #[cfg(target_os = "windows")]
    {
        win32::process_memory()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Whether the given window is the OS's foreground window. Used as a
/// supplementary signal to skip rendering when winit's focus/occluded
/// state lies (Win11 doesn't always fire `Focused(false)` when another
/// app takes fullscreen). Returns `true` on platforms where this isn't
/// implemented, so the render-skip path only triggers on Windows.
pub fn is_foreground(window: &Window) -> bool {
    #[cfg(target_os = "windows")]
    {
        win32::is_foreground(window)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        true
    }
}
