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

#[cfg(not(target_os = "windows"))]
pub use glutin_backend::GlBackend;
#[cfg(target_os = "windows")]
pub use dcomp_backend::GlBackend;

// ══════════════════════════════════════════════════════════════════════════
// Windows: Skia D3D12 + DirectComposition backend.
// ══════════════════════════════════════════════════════════════════════════
#[cfg(target_os = "windows")]
mod dcomp_backend {
    use std::cell::Cell;
    use std::sync::Arc;

    use skia_safe::gpu::d3d::{BackendContext, TextureResourceInfo};
    use skia_safe::gpu::{
        surfaces, BackendRenderTarget, DirectContext, Protected, SurfaceOrigin,
    };
    use skia_safe::{Canvas, ColorType, Surface as SkSurface};
    use winit::event_loop::ActiveEventLoop;
    use winit::window::Window;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::Interface;
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device, D3D12_COMMAND_QUEUE_DESC,
        D3D12_RESOURCE_STATE_COMMON,
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
        IDXGISwapChain3,
        DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_NONE, DXGI_ADAPTER_FLAG_SOFTWARE,
        DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
        DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
        DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };

    /// Composition swapchains use double-buffering.
    const BUFFER_COUNT: u32 = 2;
    /// BGRA to match the DComp swapchain; Skia renders premultiplied into it.
    const SWAP_FORMAT: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT =
        DXGI_FORMAT_B8G8R8A8_UNORM;

    pub struct GlBackend {
        _window: Arc<Window>,

        // Kept alive for the lifetime of the backend. Dropping the DComp
        // target tears down composition; dropping device/queue invalidates
        // the swapchain.
        _device: ID3D12Device,
        _queue: ID3D12CommandQueue,
        swap_chain: IDXGISwapChain3,
        _dcomp_device: IDCompositionDevice,
        _dcomp_target: IDCompositionTarget,
        _dcomp_visual: IDCompositionVisual,

        gr_context: DirectContext,
        /// One wrapped Skia surface per swapchain buffer, indexed by the
        /// swapchain's current-back-buffer index each frame.
        surfaces: Vec<SkSurface>,

        vsync: Cell<bool>,
        width: u32,
        height: u32,

        /// Frame counter for the periodic GPU-cache cleanup.
        frames: u64,
        /// Set once presentation hits DEVICE_REMOVED/RESET (or a resize
        /// failed) — rendering stops instead of panicking every frame.
        device_lost: bool,
    }

    impl GlBackend {
        pub fn new(_event_loop: &ActiveEventLoop, window: Arc<Window>) -> Self {
            let hwnd = hwnd_of(&window);
            let size = window.inner_size();
            let width = size.width.max(1);
            let height = size.height.max(1);

            unsafe {
                let factory: IDXGIFactory4 =
                    CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).expect("CreateDXGIFactory2");
                let (adapter, device) = hardware_adapter(&factory);
                let queue: ID3D12CommandQueue = device
                    .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default())
                    .expect("CreateCommandQueue");

                let backend_context = BackendContext {
                    adapter: adapter.clone(),
                    device: device.clone(),
                    queue: queue.clone(),
                    memory_allocator: None,
                    protected_context: Protected::No,
                };
                let mut gr_context =
                    DirectContext::new_d3d(&backend_context, None).expect("DirectContext::new_d3d");
                gr_context.set_resource_cache_limit(super::GPU_RESOURCE_CACHE_BYTES);

                // Composition swapchain — premultiplied alpha is what lets the
                // transparent corners show the desktop through DComp.
                let desc = DXGI_SWAP_CHAIN_DESC1 {
                    Width: width,
                    Height: height,
                    Format: SWAP_FORMAT,
                    Stereo: BOOL(0),
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: BUFFER_COUNT,
                    Scaling: DXGI_SCALING_STRETCH,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                    AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                    Flags: 0,
                };
                let swap_chain1: IDXGISwapChain1 = factory
                    .CreateSwapChainForComposition(&queue, &desc, None)
                    .expect("CreateSwapChainForComposition");
                let swap_chain: IDXGISwapChain3 =
                    swap_chain1.cast().expect("cast to IDXGISwapChain3");

                // DirectComposition: put the swapchain on a visual rooted to
                // the HWND. Requires the window to be WS_EX_NOREDIRECTIONBITMAP
                // (set in the launcher's window attributes) so the opaque
                // redirection surface never shows behind our alpha.
                let dcomp_device: IDCompositionDevice =
                    DCompositionCreateDevice(None::<&IDXGIDevice>)
                        .expect("DCompositionCreateDevice");
                let dcomp_target = dcomp_device
                    .CreateTargetForHwnd(hwnd, BOOL(1))
                    .expect("CreateTargetForHwnd");
                let dcomp_visual = dcomp_device.CreateVisual().expect("CreateVisual");
                dcomp_visual.SetContent(&swap_chain).expect("SetContent");
                dcomp_target.SetRoot(&dcomp_visual).expect("SetRoot");
                dcomp_device.Commit().expect("DComp Commit");

                let surfaces = wrap_surfaces(&mut gr_context, &swap_chain, width, height)
                    .expect("wrap swapchain buffers");

                log::info!(
                    "dcomp backend: D3D12 + DirectComposition swapchain {}×{}, {} buffers, premultiplied alpha",
                    width, height, BUFFER_COUNT
                );

                Self {
                    _window: window,
                    _device: device,
                    _queue: queue,
                    swap_chain,
                    _dcomp_device: dcomp_device,
                    _dcomp_target: dcomp_target,
                    _dcomp_visual: dcomp_visual,
                    gr_context,
                    surfaces,
                    vsync: Cell::new(true),
                    width,
                    height,
                    frames: 0,
                    device_lost: false,
                }
            }
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 || (width == self.width && height == self.height) {
                return;
            }
            // The wrapped surfaces reference the swapchain buffers, which
            // ResizeBuffers invalidates — drop them and let the GPU finish
            // first, then re-wrap the new buffers.
            self.surfaces.clear();
            self.gr_context.flush_submit_and_sync_cpu();
            let resized = unsafe {
                self.swap_chain.ResizeBuffers(
                    BUFFER_COUNT,
                    width,
                    height,
                    SWAP_FORMAT,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
            };
            if let Err(e) = resized {
                // Typically DXGI_ERROR_DEVICE_REMOVED (driver reset / GPU
                // unplugged). Keep the old buffers if they still wrap; else
                // stop rendering rather than panic.
                log::error!("dcomp backend: ResizeBuffers {}x{} failed: {}", width, height, e);
                let old = unsafe {
                    wrap_surfaces(&mut self.gr_context, &self.swap_chain, self.width, self.height)
                };
                match old {
                    Some(s) => self.surfaces = s,
                    None => self.mark_device_lost("resize"),
                }
                return;
            }
            match unsafe { wrap_surfaces(&mut self.gr_context, &self.swap_chain, width, height) } {
                Some(s) => self.surfaces = s,
                None => self.mark_device_lost("wrap after resize"),
            }
            self.width = width;
            self.height = height;
        }

        pub fn render<F: FnOnce(&Canvas, u32, u32)>(&mut self, draw: F) {
            if self.device_lost {
                return;
            }
            let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;
            let Some(surface) = self.surfaces.get_mut(index) else {
                return;
            };
            draw(surface.canvas(), self.width, self.height);
            // Flush + transition the buffer for present.
            self.gr_context.flush_and_submit_surface(surface, None);

            // NOTE: under DirectComposition, presentation is always composited
            // by DWM at the display refresh — there's no uncapped/tearing path
            // like a WGL swapchain had. `vsync=false` still presents every
            // frame; it just can't exceed the refresh rate. The 500fps-OLED
            // target therefore means "present every 2ms vblank", not "tear".
            let sync = if self.vsync.get() { 1 } else { 0 };
            let hr = unsafe { self.swap_chain.Present(sync, DXGI_PRESENT::default()) };
            if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
                let reason = unsafe { self._device.GetDeviceRemovedReason() };
                log::error!(
                    "dcomp backend: Present failed ({:?}, removed reason {:?})",
                    hr,
                    reason.err()
                );
                self.mark_device_lost("present");
                return;
            } else if hr.is_err() {
                log::warn!("dcomp backend: Present returned {:?}", hr);
            }

            self.frames = self.frames.wrapping_add(1);
            if self.frames.is_multiple_of(super::CLEANUP_EVERY_FRAMES) {
                self.gr_context
                    .perform_deferred_cleanup(std::time::Duration::from_secs(3), None);
            }
        }

        /// Stop rendering after an unrecoverable device error. Logged once;
        /// recreating the D3D12 device + swapchain isn't implemented, so the
        /// launcher needs a restart to draw again.
        fn mark_device_lost(&mut self, when: &str) {
            if !self.device_lost {
                log::error!(
                    "dcomp backend: GPU device lost during {} — rendering stopped (restart the launcher)",
                    when
                );
            }
            self.device_lost = true;
            self.surfaces.clear();
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

    /// Pick the first hardware (non-WARP) adapter that can create a D3D12
    /// device at feature level 11.0. Mirrors the skia-safe d3d-window example.
    fn hardware_adapter(factory: &IDXGIFactory4) -> (IDXGIAdapter1, ID3D12Device) {
        for i in 0.. {
            let adapter = unsafe { factory.EnumAdapters1(i) }.expect("EnumAdapters1");
            let desc = unsafe { adapter.GetDesc1() }.expect("GetDesc1");
            if (DXGI_ADAPTER_FLAG(desc.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                != DXGI_ADAPTER_FLAG_NONE
            {
                continue; // skip the Basic Render Driver (WARP).
            }
            let mut device: Option<ID3D12Device> = None;
            if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
                return (adapter, device.unwrap());
            }
        }
        unreachable!("no D3D12-capable hardware adapter found")
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
                let target =
                    BackendRenderTarget::new_d3d((width as i32, height as i32), &info);
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

            let _ = gl_surface.set_swap_interval(
                &gl_context,
                SwapInterval::Wait(NonZeroU32::new(1).unwrap()),
            );

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

        pub fn set_vsync(&self, enabled: bool) {
            let interval = if enabled {
                SwapInterval::Wait(NonZeroU32::new(1).unwrap())
            } else {
                SwapInterval::DontWait
            };
            let _ = self.gl_surface.set_swap_interval(&self.gl_context, interval);
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
