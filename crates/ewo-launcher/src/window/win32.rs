//! Windows custom-frame implementation.
//!
//! Step 1 implementation: applies `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND`
//! so DWM rounds the outer corners on Win11. Combined with
//! `Window::with_decorations(false)`, this gives us a borderless rounded window
//! with the OS-provided shadow.
//!
//! Drag and resize hit-testing live in `main.rs` and route through
//! `Window::drag_window()` / `Window::drag_resize_window(dir)`, which winit
//! translates into `WM_NCLBUTTONDOWN` posts internally. We don't subclass the
//! window proc — winit owns it.
//!
//! When the launcher's own painted box-shadow lands (step 2+), we may want to
//! disable the DWM shadow with `DWMSBT_DISABLE` to avoid double-shadowing.
//! That's a step-2 concern, not step 1.

#![cfg(target_os = "windows")]

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

pub fn configure(window: &Window) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    let raw = match window.window_handle() {
        Ok(h) => h.as_raw(),
        Err(e) => {
            log::warn!("could not retrieve window handle: {e}");
            return;
        }
    };

    let hwnd = match raw {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        _ => {
            log::warn!("non-Win32 window handle on a Windows build");
            return;
        }
    };

    let pref: u32 = DWMWCP_ROUND.0 as u32;
    let res = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if let Err(e) = res {
        log::warn!("DwmSetWindowAttribute(corner_preference) failed: {e:?}");
    }
}
