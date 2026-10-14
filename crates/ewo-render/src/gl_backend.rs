//! Rendering backend — Skia on the GPU, presented to a winit window.
//!
//! Two platform backends live behind one public `GlBackend` API
//! (`new` / `resize` / `render` / `set_vsync`):
//!
//! - **Windows (`dcomp_backend`)**: Skia's **Direct3D 12** backend rendering
//!   into a **DirectComposition** swapchain with premultiplied alpha, presented
//!   through a DComp visual on a `WS_EX_NOREDIRECTIONBITMAP` window. This is the
//!   only reliable way to get true per-pixel-alpha rounded corners on Win11 — a
//!   WGL/GL swapchain's alpha is composited opaque by DWM no matter what DWM
//!   attributes we set, which is what produced the black corners.
//! - **Everything else (`glutin_backend`)**: Skia's GL backend on a glutin
//!   window surface. Unchanged from the original step-2 implementation. This is
//!   the Hyprland/Wayland path.
//!
//! The name `GlBackend` is kept on both so `main.rs` is platform-agnostic.

/// Cap on Skia's GPU resource cache (both backends). Skia's default is
/// 256 MB; the launcher's working set is far smaller, so a tighter cap just
/// bounds long-session growth.
const GPU_RESOURCE_CACHE_BYTES: usize = 192 * 1024 * 1024;

/// Every this many frames, let Skia free GPU resources unused for a few
/// seconds (it otherwise only purges when the cache limit is hit).
const CLEANUP_EVERY_FRAMES: u64 = 300;

/// Bumped every time the GPU context is recreated (device loss). Skia images
/// cached across frames belong to one context; caches store the generation
/// they were built under and rebuild when it no longer matches.
static GPU_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The current GPU context generation. See [`GPU_GENERATION`].
pub fn gpu_generation() -> u64 {
    GPU_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn bump_gpu_generation() {
    GPU_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Tell the user the launcher cannot start, where they will see it.
///
/// The release launcher is a windowed app (`windows_subsystem = "windows"`), so
/// a `panic!` or a `log::error!` on a machine with no usable GPU is invisible:
/// the process just vanishes. A native message box is the one channel that
/// always reaches the user. Elsewhere there is a terminal, so stderr will do.
pub fn show_fatal_error(title: &str, message: &str) {
    log::error!("{title}: {message}");
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
        };
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
        let (title, message) = (wide(title), wide(message));
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    eprintln!("{title}: {message}");
}

#[cfg(target_os = "windows")]
pub use dcomp_backend::GlBackend;
#[cfg(not(target_os = "windows"))]
pub use glutin_backend::GlBackend;

// ══════════════════════════════════════════════════════════════════════════
// Windows: Skia D3D12 + DirectComposition backend.
// ══════════════════════════════════════════════════════════════════════════
#[cfg(target_os = "windows")]
mod dcomp_backend {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use skia_safe::gpu::d3d::{BackendContext, TextureResourceInfo};
    use skia_safe::gpu::{
        backend_render_targets, direct_contexts, surfaces, DirectContext, Protected, SurfaceOrigin,
    };
    use skia_safe::{Canvas, ColorType, Surface as SkSurface};
    use winit::event_loop::ActiveEventLoop;
    use winit::window::Window;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::{Interface, BOOL};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device, ID3D12Device5,
        D3D12_COMMAND_QUEUE_DESC, D3D12_RESOURCE_STATE_COMMON,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, IDXGIAdapter1, IDXGIDevice, IDXGIFactory4, IDXGISwapChain1,
        IDXGISwapChain3, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_NONE, DXGI_ADAPTER_FLAG_SOFTWARE,
        DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
        DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
        DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };

    /// Composition swapchains use double-buffering.
    const BUFFER_COUNT: u32 = 2;
    /// BGRA to match the DComp swapchain; Skia renders premultiplied into it.
    const SWAP_FORMAT: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT =
        DXGI_FORMAT_B8G8R8A8_UNORM;
    /// Recreation backoff after a device loss: 0.5 s, doubling, capped here.
    const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

    /// Everything that belongs to one D3D12 device, dropped and rebuilt as a
    /// unit when the device is lost.
    struct DeviceStack {
        // Field order is drop order: Skia first (its surfaces wrap the
        // swapchain buffers), then composition, the swapchain, and last the
        // device the swapchain was created on.
        surfaces: Vec<SkSurface>,
        gr_context: DirectContext,
        _dcomp_visual: IDCompositionVisual,
        _dcomp_target: IDCompositionTarget,
        _dcomp_device: IDCompositionDevice,
        swap_chain: IDXGISwapChain3,
        _queue: ID3D12CommandQueue,
        device: ID3D12Device,
        /// Whether this stack runs on the WARP software adapter.
        on_warp: bool,
    }

    fn werr(what: &'static str) -> impl FnOnce(windows::core::Error) -> String {
        move |err| format!("{what}: {err}")
    }

    impl DeviceStack {
        /// `allow_warp`: whether the WARP software adapter may be used when no
        /// hardware adapter works (see [`select_adapter`]).
        unsafe fn create(
            hwnd: HWND,
            width: u32,
            height: u32,
            allow_warp: bool,
        ) -> Result<Self, String> {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(werr("CreateDXGIFactory2"))?;
            let (adapter, device, on_warp) = pick_adapter(&factory, allow_warp)?;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default())
                .map_err(werr("CreateCommandQueue"))?;

            let backend_context = BackendContext {
                adapter: adapter.clone(),
                device: device.clone(),
                queue: queue.clone(),
                memory_allocator: None,
                protected_context: Protected::No,
            };
            let mut gr_context = direct_contexts::make_d3d(&backend_context, None)
                .ok_or("direct_contexts::make_d3d failed")?;
            gr_context.set_resource_cache_limit(super::GPU_RESOURCE_CACHE_BYTES);

            // Composition swapchain — premultiplied alpha is what lets the
            // transparent corners show the desktop through DComp.
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: SWAP_FORMAT,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: BUFFER_COUNT,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap_chain1: IDXGISwapChain1 = factory
                .CreateSwapChainForComposition(&queue, &desc, None)
                .map_err(werr("CreateSwapChainForComposition"))?;
            let swap_chain: IDXGISwapChain3 = swap_chain1
                .cast()
                .map_err(werr("cast to IDXGISwapChain3"))?;

            // DirectComposition: put the swapchain on a visual rooted to the
            // HWND. Requires the window to be WS_EX_NOREDIRECTIONBITMAP (set in
            // the launcher's window attributes) so the opaque redirection
            // surface never shows behind our alpha. A window takes one target
            // at a time, so a previous stack must already be dropped.
            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(None::<&IDXGIDevice>)
                .map_err(werr("DCompositionCreateDevice"))?;
            let dcomp_target = dcomp_device
                .CreateTargetForHwnd(hwnd, true)
                .map_err(werr("CreateTargetForHwnd"))?;
            let dcomp_visual = dcomp_device.CreateVisual().map_err(werr("CreateVisual"))?;
            dcomp_visual
                .SetContent(&swap_chain)
                .map_err(werr("SetContent"))?;
            dcomp_target
                .SetRoot(&dcomp_visual)
                .map_err(werr("SetRoot"))?;
            dcomp_device.Commit().map_err(werr("DComp Commit"))?;

            let surfaces = wrap_surfaces(&mut gr_context, &swap_chain, width, height)
                .ok_or("wrap swapchain buffers failed")?;
            Ok(Self {
                surfaces,
                gr_context,
                _dcomp_visual: dcomp_visual,
                _dcomp_target: dcomp_target,
                _dcomp_device: dcomp_device,
                swap_chain,
                _queue: queue,
                device,
                on_warp,
            })
        }

        /// Tear down after a device loss. Skia must not touch the dead device
        /// again, so its context is abandoned rather than flushed.
        fn abandon(mut self) {
            self.surfaces.clear();
            self.gr_context.abandon();
        }
    }

    /// Rendering is paused until `next_try`.
    struct Lost {
        attempts: u32,
        next_try: Instant,
    }

    pub struct GlBackend {
        window: Arc<Window>,
        stack: Option<DeviceStack>,
        lost: Option<Lost>,
        /// The session's first device landed on WARP (no hardware adapter at
        /// startup). Only then may a recreation fall back to WARP again; a
        /// session that started on hardware never silently drops to software
        /// because a TDR briefly broke `D3D12CreateDevice`.
        started_on_warp: bool,
        vsync: Cell<bool>,
        width: u32,
        height: u32,
        /// Frame counter for the periodic GPU-cache cleanup.
        frames: u64,
        /// Debug builds only: `EWO_SIMULATE_DEVICE_LOSS=<frames>` removes the
        /// device after that many frames, to exercise recovery for real.
        simulate_loss_at: Option<u64>,
    }

    impl GlBackend {
        /// Create the backend, or show a native error box and exit if no
        /// Direct3D 12 device (not even WARP) can be created: the launcher has
        /// nothing to draw with, and a windowed release build would otherwise
        /// vanish without a word. Use [`Self::try_new`] to handle the error.
        pub fn new(event_loop: &ActiveEventLoop, window: Arc<Window>) -> Self {
            match Self::try_new(event_loop, window) {
                Ok(backend) => backend,
                Err(e) => {
                    super::show_fatal_error(
                        "EwoClient can't start",
                        &format!(
                            "EwoClient could not set up graphics.\n\n{e}\n\n\
                             Update your graphics driver, or run it on a machine with \
                             Direct3D 12 support."
                        ),
                    );
                    std::process::exit(1);
                }
            }
        }

        pub fn try_new(_event_loop: &ActiveEventLoop, window: Arc<Window>) -> Result<Self, String> {
            let size = window.inner_size();
            let width = size.width.max(1);
            let height = size.height.max(1);
            // Startup is the only time WARP is a legitimate last resort.
            let allow_warp = warp_allowed(AdapterPhase::Startup, false);
            let stack = unsafe { DeviceStack::create(hwnd_of(&window), width, height, allow_warp) }
                .map_err(|e| format!("initial device creation failed: {e}"))?;
            let started_on_warp = stack.on_warp;
            log::info!(
                "dcomp backend: D3D12 + DirectComposition swapchain {}×{}, {} buffers, premultiplied alpha",
                width, height, BUFFER_COUNT
            );
            let simulate_loss_at = if cfg!(debug_assertions) {
                std::env::var("EWO_SIMULATE_DEVICE_LOSS")
                    .ok()
                    .and_then(|v| v.parse().ok())
            } else {
                None
            };
            Ok(Self {
                window,
                stack: Some(stack),
                lost: None,
                started_on_warp,
                vsync: Cell::new(true),
                width,
                height,
                frames: 0,
                simulate_loss_at,
            })
        }

        /// While the device is lost: when the next recreation attempt is due.
        /// The event loop should sleep until then instead of polling (there
        /// is nothing to draw), and `None` means rendering is healthy.
        pub fn retry_deadline(&self) -> Option<Instant> {
            self.lost.as_ref().map(|l| l.next_try)
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 || (width == self.width && height == self.height) {
                return;
            }
            self.width = width;
            self.height = height;
            // While lost, the rebuild picks up the new size.
            let Some(stack) = self.stack.as_mut() else {
                return;
            };
            // The device can die between two frames without `Present` having
            // said so yet (a driver reset during a window drag). Waiting on a
            // removed device's fence is undefined, so find out first.
            if let Err(reason) = unsafe { stack.device.GetDeviceRemovedReason() } {
                log::error!("dcomp backend: device already removed at resize ({reason})");
                self.device_lost("resize (device removed)");
                return;
            }
            // The wrapped surfaces reference the swapchain buffers, which
            // ResizeBuffers invalidates: drop them and let the GPU finish
            // first, then re-wrap the new buffers.
            stack.surfaces.clear();
            stack.gr_context.flush_submit_and_sync_cpu();
            let resized = unsafe {
                stack.swap_chain.ResizeBuffers(
                    BUFFER_COUNT,
                    width,
                    height,
                    SWAP_FORMAT,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
            };
            if let Err(e) = resized {
                log::error!(
                    "dcomp backend: ResizeBuffers {}x{} failed: {}",
                    width,
                    height,
                    e
                );
                self.device_lost("resize");
                return;
            }
            match unsafe { wrap_surfaces(&mut stack.gr_context, &stack.swap_chain, width, height) }
            {
                Some(s) => stack.surfaces = s,
                None => self.device_lost("wrap after resize"),
            }
        }

        pub fn render<F: FnOnce(&Canvas, u32, u32)>(&mut self, draw: F) {
            if self.stack.is_none() && !self.try_recreate() {
                return;
            }
            let Some(stack) = self.stack.as_mut() else {
                return;
            };
            let index = unsafe { stack.swap_chain.GetCurrentBackBufferIndex() } as usize;
            let Some(surface) = stack.surfaces.get_mut(index) else {
                return;
            };
            draw(surface.canvas(), self.width, self.height);
            // Flush + transition the buffer for present.
            stack.gr_context.flush_and_submit_surface(surface, None);

            // NOTE: under DirectComposition, presentation is always composited
            // by DWM at the display refresh — there's no uncapped/tearing path
            // like a WGL swapchain had. `vsync=false` still presents every
            // frame; it just can't exceed the refresh rate. The 500fps-OLED
            // target therefore means "present every 2ms vblank", not "tear".
            let sync = if self.vsync.get() { 1 } else { 0 };
            let hr = unsafe { stack.swap_chain.Present(sync, DXGI_PRESENT::default()) };
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                let reason = unsafe { stack.device.GetDeviceRemovedReason() };
                log::error!(
                    "dcomp backend: Present failed ({:?}, removed reason {:?})",
                    hr,
                    reason.err()
                );
                self.device_lost("present");
                return;
            } else if hr.is_err() {
                log::warn!("dcomp backend: Present returned {:?}", hr);
            }

            self.frames = self.frames.wrapping_add(1);
            if self.frames.is_multiple_of(super::CLEANUP_EVERY_FRAMES) {
                stack
                    .gr_context
                    .perform_deferred_cleanup(Duration::from_secs(3), None);
            }
            if self.simulate_loss_at == Some(self.frames) {
                log::warn!("dcomp backend: EWO_SIMULATE_DEVICE_LOSS — removing the device");
                if let Ok(d5) = stack.device.cast::<ID3D12Device5>() {
                    unsafe { d5.RemoveDevice() };
                }
            }
        }

        /// Drop the whole device stack after an unrecoverable device error;
        /// `render` rebuilds it with backoff.
        fn device_lost(&mut self, when: &str) {
            if let Some(stack) = self.stack.take() {
                log::error!("dcomp backend: GPU device lost during {when} — recreating");
                stack.abandon();
            }
            self.lost.get_or_insert(Lost {
                attempts: 0,
                next_try: Instant::now(),
            });
        }

        /// Rebuild the device stack if the backoff allows. Returns whether a
        /// stack is available afterwards.
        fn try_recreate(&mut self) -> bool {
            let Some(lost) = self.lost.as_mut() else {
                return false;
            };
            if Instant::now() < lost.next_try {
                return false;
            }
            lost.attempts += 1;
            let allow_warp = warp_allowed(AdapterPhase::Recreate, self.started_on_warp);
            match unsafe {
                DeviceStack::create(hwnd_of(&self.window), self.width, self.height, allow_warp)
            } {
                Ok(stack) => {
                    log::info!(
                        "dcomp backend: device recreated after {} attempt(s)",
                        lost.attempts
                    );
                    self.stack = Some(stack);
                    self.lost = None;
                    // Images cached under the old context are unusable now.
                    super::bump_gpu_generation();
                    true
                }
                Err(e) => {
                    let delay = Duration::from_millis(500)
                        .saturating_mul(1 << lost.attempts.min(6))
                        .min(MAX_RETRY_DELAY);
                    log::warn!(
                        "dcomp backend: recreate attempt {} failed ({e}); retrying in {:?}",
                        lost.attempts,
                        delay
                    );
                    lost.next_try = Instant::now() + delay;
                    false
                }
            }
        }

        /// See the note in `render` — under DComp this only toggles the
        /// present sync interval; it can't uncap past the refresh rate.
        pub fn set_vsync(&self, enabled: bool) {
            self.vsync.set(enabled);
        }
    }

    /// Resolve the Win32 HWND from a winit window.
    fn hwnd_of(window: &Window) -> HWND {
        match window.window_handle().expect("window_handle").as_raw() {
            RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
            _ => panic!("non-Win32 window handle on a Windows build"),
        }
    }

    /// The first hardware adapter that can create a D3D12 device at feature
    /// level 11.0 (mirrors the skia-safe d3d-window example), else, when
    /// `allow_warp`, the WARP software adapter with a warning: slow, but a
    /// machine with no D3D12 GPU (a VM, a broken driver, Remote Desktop) can
    /// still show the launcher. Returns the adapter, device and whether it is
    /// WARP.
    fn pick_adapter(
        factory: &IDXGIFactory4,
        allow_warp: bool,
    ) -> Result<(IDXGIAdapter1, ID3D12Device, bool), String> {
        let picked = select_adapter(
            allow_warp,
            || enumerate_adapter(factory, false),
            || enumerate_adapter(factory, true),
        );
        match picked {
            Some((AdapterKind::Hardware, (adapter, device))) => Ok((adapter, device, false)),
            Some((AdapterKind::Warp, (adapter, device))) => {
                log::warn!(
                    "dcomp backend: no D3D12 hardware adapter, falling back to WARP \
                     (software rendering, expect a low frame rate)"
                );
                Ok((adapter, device, true))
            }
            None if allow_warp => {
                Err("no Direct3D 12 adapter, hardware or WARP, could create a device".into())
            }
            None => Err("no Direct3D 12 hardware adapter could create a device \
                         (WARP is not used after startup)"
                .into()),
        }
    }

    /// When an adapter is being picked: at process startup, or while
    /// rebuilding after a device loss.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum AdapterPhase {
        Startup,
        Recreate,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum AdapterKind {
        Hardware,
        Warp,
    }

    /// WARP policy. Startup may always fall back to it (a machine with no
    /// D3D12 GPU still shows the launcher). A recreation after a TDR / driver
    /// reset may use it only if the session already started on WARP: hardware
    /// `D3D12CreateDevice` failing for a moment must not "recover" onto
    /// software rendering for the rest of the session; the backoff retries
    /// hardware instead.
    fn warp_allowed(phase: AdapterPhase, started_on_warp: bool) -> bool {
        phase == AdapterPhase::Startup || started_on_warp
    }

    /// The selection order as a pure function: hardware first, then WARP only
    /// if `allow_warp`.
    fn select_adapter<T>(
        allow_warp: bool,
        hardware: impl FnOnce() -> Option<T>,
        warp: impl FnOnce() -> Option<T>,
    ) -> Option<(AdapterKind, T)> {
        if let Some(found) = hardware() {
            return Some((AdapterKind::Hardware, found));
        }
        if allow_warp {
            return warp().map(|found| (AdapterKind::Warp, found));
        }
        None
    }

    /// The first adapter whose software flag equals `software` that creates a
    /// D3D12 device.
    fn enumerate_adapter(
        factory: &IDXGIFactory4,
        software: bool,
    ) -> Option<(IDXGIAdapter1, ID3D12Device)> {
        for i in 0.. {
            // EnumAdapters1 fails (DXGI_ERROR_NOT_FOUND) past the last adapter.
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
                break;
            };
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            let is_software = (DXGI_ADAPTER_FLAG(desc.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                != DXGI_ADAPTER_FLAG_NONE;
            if is_software != software {
                continue; // the Basic Render Driver (WARP) is the fallback pass.
            }
            let mut device: Option<ID3D12Device> = None;
            if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
                if let Some(device) = device {
                    return Some((adapter, device));
                }
            }
        }
        None
    }

    /// Wrap each swapchain back buffer as a Skia surface. Called at creation
    /// and after every resize.
    unsafe fn wrap_surfaces(
        gr_context: &mut DirectContext,
        swap_chain: &IDXGISwapChain3,
        width: u32,
        height: u32,
    ) -> Option<Vec<SkSurface>> {
        (0..BUFFER_COUNT)
            .map(|i| {
                let resource = match swap_chain.GetBuffer(i) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("dcomp backend: swapchain GetBuffer({}) failed: {}", i, e);
                        return None;
                    }
                };
                let info = TextureResourceInfo {
                    resource,
                    alloc: None,
                    resource_state: D3D12_RESOURCE_STATE_COMMON,
                    format: SWAP_FORMAT,
                    sample_count: 1,
                    level_count: 1,
                    sample_quality_pattern: DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
                    protected: Protected::No,
                };
                let target = backend_render_targets::make_d3d((width as i32, height as i32), &info);
                surfaces::wrap_backend_render_target(
                    gr_context,
                    &target,
                    // D3D render targets are top-left origin (unlike GL).
                    SurfaceOrigin::TopLeft,
                    // BGRA matches the swapchain format — no channel swizzle.
                    ColorType::BGRA8888,
                    None,
                    None,
                )
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn factory() -> IDXGIFactory4 {
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.expect("DXGI factory")
        }

        /// The fallback the launcher relies on when there is no D3D12 GPU:
        /// Windows' software adapter must be enumerable and able to make a
        /// device, and must be flagged as software (so the hardware pass
        /// skips it and the fallback pass finds it).
        #[test]
        fn warp_is_the_software_fallback_adapter() {
            let (adapter, _device) = enumerate_adapter(&factory(), true)
                .expect("no WARP adapter: the no-GPU fallback would fail");
            let desc = unsafe { adapter.GetDesc1() }.expect("adapter desc");
            assert!(
                (DXGI_ADAPTER_FLAG(desc.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                    != DXGI_ADAPTER_FLAG_NONE
            );
        }

        /// The hardware pass never returns a software adapter, and
        /// `pick_adapter` succeeds on any machine that has either kind.
        #[test]
        fn hardware_pass_skips_software_and_pick_adapter_succeeds() {
            if let Some((adapter, _)) = enumerate_adapter(&factory(), false) {
                let desc = unsafe { adapter.GetDesc1() }.expect("adapter desc");
                assert!(
                    (DXGI_ADAPTER_FLAG(desc.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                        == DXGI_ADAPTER_FLAG_NONE
                );
            }
            assert!(pick_adapter(&factory(), true).is_ok());
        }

        #[test]
        fn warp_policy_startup_vs_recreate() {
            assert!(warp_allowed(AdapterPhase::Startup, false));
            assert!(warp_allowed(AdapterPhase::Startup, true));
            // A hardware session never drops to software on recreation.
            assert!(!warp_allowed(AdapterPhase::Recreate, false));
            // A session that started on WARP may keep using it.
            assert!(warp_allowed(AdapterPhase::Recreate, true));
        }

        #[test]
        fn select_adapter_prefers_hardware_and_gates_warp() {
            let hw = || Some("hw");
            let warp = || Some("warp");
            let none = || None::<&str>;
            assert_eq!(
                select_adapter(true, hw, warp),
                Some((AdapterKind::Hardware, "hw"))
            );
            assert_eq!(
                select_adapter(false, hw, warp),
                Some((AdapterKind::Hardware, "hw"))
            );
            assert_eq!(
                select_adapter(true, none, warp),
                Some((AdapterKind::Warp, "warp"))
            );
            // No hardware, WARP disallowed: fail (the backoff retries) and
            // never even probe WARP.
            assert_eq!(
                select_adapter(false, none, || panic!("WARP probed when disallowed")),
                None
            );
            assert_eq!(select_adapter(true, none, none), None);
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Non-Windows (Linux/Hyprland): Skia GL on a glutin window surface.
// ══════════════════════════════════════════════════════════════════════════
#[cfg(not(target_os = "windows"))]
mod glutin_backend {
    use std::ffi::CString;
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use glutin::config::{ConfigTemplateBuilder, GlConfig};
    use glutin::context::{
        ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, Version,
    };
    use glutin::display::{GetGlDisplay, GlDisplay};
    use glutin::prelude::GlSurface;
    use glutin::surface::{Surface as GlSurfaceT, SwapInterval, WindowSurface};
    use glutin_winit::DisplayBuilder;
    use raw_window_handle::HasWindowHandle;
    use skia_safe::gpu::backend_render_targets;
    use skia_safe::gpu::direct_contexts;
    use skia_safe::gpu::gl::{Format, FramebufferInfo, Interface};
    use skia_safe::gpu::{surfaces, DirectContext, SurfaceOrigin};
    use skia_safe::{Canvas, ColorType, Surface as SkSurface};
    use winit::event_loop::ActiveEventLoop;
    use winit::window::Window;

    pub struct GlBackend {
        /// Held to keep the window alive for the GL surface.
        _window: Arc<Window>,

        gl_surface: GlSurfaceT<WindowSurface>,
        gl_context: PossiblyCurrentContext,

        gr_context: DirectContext,
        fb_info: FramebufferInfo,
        sample_count: usize,
        stencil_size: usize,

        sk_surface: SkSurface,
        width: u32,
        height: u32,

        /// Frame counter for the periodic GPU-cache cleanup.
        frames: u64,
    }

    impl GlBackend {
        pub fn new(event_loop: &ActiveEventLoop, window: Arc<Window>) -> Self {
            let (gl_display, gl_config) = {
                let template = ConfigTemplateBuilder::new()
                    .with_alpha_size(8)
                    .with_stencil_size(8);

                let display_builder = DisplayBuilder::new();
                let (_w, gl_config) = display_builder
                    .build(event_loop, template, |configs| {
                        configs
                            .reduce(|acc, c| {
                                let acc_has_alpha = acc.alpha_size() > 0;
                                let c_has_alpha = c.alpha_size() > 0;
                                match (acc_has_alpha, c_has_alpha) {
                                    (true, false) => acc,
                                    (false, true) => c,
                                    _ => {
                                        if c.num_samples() > acc.num_samples() {
                                            c
                                        } else {
                                            acc
                                        }
                                    }
                                }
                            })
                            .expect("no GL config")
                    })
                    .expect("DisplayBuilder::build failed");
                let gl_display = gl_config.display();
                log::info!(
                    "gl config: alpha={} bits, samples={}, stencil={}",
                    gl_config.alpha_size(),
                    gl_config.num_samples(),
                    gl_config.stencil_size(),
                );
                (gl_display, gl_config)
            };

            let raw_window_handle = window.window_handle().expect("window_handle").as_raw();

            let context_attrs = ContextAttributesBuilder::new()
                .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
                .build(Some(raw_window_handle));

            let not_current_context = unsafe {
                gl_display
                    .create_context(&gl_config, &context_attrs)
                    .expect("create_context")
            };

            let size = window.inner_size();
            let surface_attrs = glutin::surface::SurfaceAttributesBuilder::<WindowSurface>::new()
                .build(
                    raw_window_handle,
                    NonZeroU32::new(size.width.max(1)).unwrap(),
                    NonZeroU32::new(size.height.max(1)).unwrap(),
                );

            let gl_surface = unsafe {
                gl_display
                    .create_window_surface(&gl_config, &surface_attrs)
                    .expect("create_window_surface")
            };

            let gl_context = not_current_context
                .make_current(&gl_surface)
                .expect("make_current");

            let _ = gl_surface
                .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

            gl::load_with(|s| {
                let cstr = CString::new(s).unwrap();
                gl_display.get_proc_address(&cstr) as *const _
            });

            let interface = Interface::new_load_with(|name| {
                if name == "eglGetCurrentDisplay" {
                    return std::ptr::null();
                }
                let cstr = match CString::new(name) {
                    Ok(c) => c,
                    Err(_) => return std::ptr::null(),
                };
                gl_display.get_proc_address(&cstr) as *const _
            })
            .expect("Interface::new_load_with");

            let mut gr_context =
                direct_contexts::make_gl(interface, None).expect("direct_contexts::make_gl");

            gr_context.set_resource_cache_limit(super::GPU_RESOURCE_CACHE_BYTES);

            let fb_info = {
                let mut fboid: gl::types::GLint = 0;
                unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fboid) };
                FramebufferInfo {
                    fboid: fboid.try_into().unwrap_or(0),
                    format: Format::RGBA8.into(),
                    ..Default::default()
                }
            };

            let sample_count = gl_config.num_samples() as usize;
            let stencil_size = gl_config.stencil_size() as usize;

            let sk_surface = create_surface(
                &mut gr_context,
                fb_info,
                sample_count,
                stencil_size,
                size.width,
                size.height,
            );

            Self {
                _window: window,
                gl_surface,
                gl_context,
                gr_context,
                fb_info,
                sample_count,
                stencil_size,
                sk_surface,
                width: size.width,
                height: size.height,
                frames: 0,
            }
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 {
                return;
            }
            let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
                return;
            };
            self.gl_surface.resize(&self.gl_context, w, h);
            self.width = width;
            self.height = height;
            self.sk_surface = create_surface(
                &mut self.gr_context,
                self.fb_info,
                self.sample_count,
                self.stencil_size,
                width,
                height,
            );
        }

        pub fn render<F: FnOnce(&Canvas, u32, u32)>(&mut self, draw: F) {
            let canvas = self.sk_surface.canvas();
            draw(canvas, self.width, self.height);
            self.gr_context.flush_and_submit();
            let _ = self.gl_surface.swap_buffers(&self.gl_context);

            self.frames = self.frames.wrapping_add(1);
            if self.frames.is_multiple_of(super::CLEANUP_EVERY_FRAMES) {
                self.gr_context
                    .perform_deferred_cleanup(std::time::Duration::from_secs(3), None);
            }
        }

        /// GL has no device-loss recovery to wait for (see the Windows
        /// backend's `retry_deadline`).
        pub fn retry_deadline(&self) -> Option<std::time::Instant> {
            None
        }

        pub fn set_vsync(&self, enabled: bool) {
            let interval = if enabled {
                SwapInterval::Wait(NonZeroU32::new(1).unwrap())
            } else {
                SwapInterval::DontWait
            };
            let _ = self
                .gl_surface
                .set_swap_interval(&self.gl_context, interval);
        }
    }

    fn create_surface(
        gr_context: &mut DirectContext,
        fb_info: FramebufferInfo,
        sample_count: usize,
        stencil_size: usize,
        width: u32,
        height: u32,
    ) -> SkSurface {
        let backend_render_target = backend_render_targets::make_gl(
            (width as i32, height as i32),
            sample_count,
            stencil_size,
            fb_info,
        );

        surfaces::wrap_backend_render_target(
            gr_context,
            &backend_render_target,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            None,
            None,
        )
        .expect("wrap_backend_render_target")
    }
}
