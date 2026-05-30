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

/// Returns `true` when the given window is the OS's foreground window —
/// the one the user is actively interacting with. winit's
/// `WindowEvent::Focused` is unreliable on Win11 (it doesn't always fire
/// when another app takes fullscreen / takes the foreground), so we poll
/// `GetForegroundWindow()` directly to drive the render-skip decision.
/// Returns `false` if the window handle can't be resolved.
pub fn is_foreground(window: &Window) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let raw = match window.window_handle() {
        Ok(h) => h.as_raw(),
        Err(_) => return false,
    };
    let my_hwnd = match raw {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        _ => return false,
    };
    let fg = unsafe { GetForegroundWindow() };
    fg.0 == my_hwnd.0
}

/// LEAK_HUNT_INSTRUMENT — strip before release.
/// Current process memory snapshot — `(working_set_bytes, private_bytes)`.
/// `working_set` is the actively-mapped RSS the kernel reports back to
/// Task Manager's "Memory" column. `private_bytes` is committed
/// process-private memory (the "Memory (private working set)" column).
/// Used by the launcher's periodic memory-growth diagnostic. If this
/// function goes away, also remove `Win32_System_Threading` +
/// `Win32_System_ProcessStatus` from the workspace `windows` features.
pub fn process_memory() -> Option<(u64, u64)> {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let proc = unsafe { GetCurrentProcess() };
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let res = unsafe {
        GetProcessMemoryInfo(
            proc,
            &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            size,
        )
    };
    if res.is_err() {
        return None;
    }
    Some((counters.WorkingSetSize as u64, counters.PrivateUsage as u64))
}
