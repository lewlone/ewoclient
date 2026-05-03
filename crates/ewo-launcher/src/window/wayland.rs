//! Wayland custom-frame implementation, targeting Hyprland (wlroots).
//!
//! Step 1 (TODO):
//!
//! 1. Declare client-side decoration: `zxdg_toplevel_decoration_v1.set_mode(client_side)`.
//!    On winit 0.30 this happens automatically when `with_decorations(false)`
//!    is passed and the compositor advertises `xdg-decoration`. Hyprland does.
//! 2. Drag: on a click in the drag region, call `Window::drag_window()` —
//!    winit translates this into `xdg_toplevel.move(serial)`.
//! 3. Resize: on a click in an edge zone, call `Window::drag_resize_window(edge)` —
//!    winit translates this into `xdg_toplevel.resize(serial, edges)`.
//! 4. Rounded corners and drop shadow are painted by us into the alpha buffer
//!    (Hyprland respects window alpha). No compositor cooperation needed.
//! 5. Fractional scaling: winit honors `wp-fractional-scale-v1` and reports
//!    `scale_factor` via the resize event. Pass through to Skia surface.
//!
//! No X11 implementation — CachyOS/Hyprland is the only Linux target.
//!
//! References:
//! - https://wayland.app/protocols/xdg-shell
//! - https://wayland.app/protocols/xdg-decoration-unstable-v1

#![cfg(target_os = "linux")]

use winit::window::Window;

pub fn configure(_window: &Window) {
    // TODO (step 1):
    //   - Verify decoration-mode came back as client-side
    //   - Wire drag-region click → window.drag_window()
    //   - Wire edge-region click → window.drag_resize_window(edge)
}
