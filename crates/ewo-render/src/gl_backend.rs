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

// ╔═══════════════════════════════════════════════════════════════════════╗
// ║ LEAK_HUNT_INSTRUMENT — strip before release.                         ║
// ║                                                                       ║
// ║ Skia cache caps + diagnostic helpers below were added during the     ║
// ║ memory-leak hunt. The actual leak turned out to be unrelated —       ║
// ║ `wglSwapBuffers` on a fullscreen-occluded window leaking driver-     ║
// ║ side present queue memory (~6 KB/frame). Fix lives in                ║
// ║ `main.rs::WindowEvent::RedrawRequested` (skip render when not the    ║
// ║ foreground window). These caps are harmless insurance but not        ║
// ║ needed for correctness. The periodic log in `render()` is the same.  ║
// ╚═══════════════════════════════════════════════════════════════════════╝

/// Cap Skia's *process-wide* (CPU-side) caches. These live in
/// `SkGraphics::SetResourceCacheTotalByteLimit` / `SetFontCacheLimit` and
/// are separate from the `DirectContext`'s GPU resource cache. Defaults
/// in Skia are 32 MB / 256 MB respectively; cap tighter so a long-running
/// session can't spend memory on bitmap/glyph rasterisation history we
/// don't actually need.
///
/// Call once at process startup, before any `GlBackend::new`.
pub fn cap_skia_global_caches() {
    let prev_res =
        skia_safe::graphics::set_resource_cache_total_bytes_limit(64 * 1024 * 1024);
    let prev_font = skia_safe::graphics::set_font_cache_limit(96 * 1024 * 1024);
    log::info!(
        "skia globals: resource cache {} → 64 MB, font cache {} → 96 MB",
        format_bytes(prev_res),
        format_bytes(prev_font),
    );
}

/// Log current Skia CPU-cache usage. The launcher's periodic memory
/// diagnostic calls this alongside its RSS log so we can tell whether the
/// global resource cache + font cache are climbing.
pub fn log_skia_global_cache_state() {
    let res_used = skia_safe::graphics::resource_cache_total_bytes_used();
    let res_lim = skia_safe::graphics::resource_cache_total_bytes_limit();
    let font_used = skia_safe::graphics::font_cache_used();
    let font_lim = skia_safe::graphics::font_cache_limit();
    let font_count = skia_safe::graphics::font_cache_count_used();
    log::info!(
        "skia globals: resource {}/{}, font {}/{} ({} strikes)",
        format_bytes(res_used),
        format_bytes(res_lim),
        format_bytes(font_used),
        format_bytes(font_lim),
        font_count,
    );
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
// ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────────────────

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
        DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
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

        /// LEAK_HUNT_INSTRUMENT — strip before release. Frame counter for the
        /// periodic GPU-cache cleanup + diagnostic log.
        frames: u64,
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

                let surfaces = wrap_surfaces(&mut gr_context, &swap_chain, width, height);

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
            unsafe {
                self.swap_chain
                    .ResizeBuffers(BUFFER_COUNT, width, height, SWAP_FORMAT, DXGI_SWAP_CHAIN_FLAG(0))
                    .expect("ResizeBuffers");
                self.surfaces = wrap_surfaces(&mut self.gr_context, &self.swap_chain, width, height);
            }
            self.width = width;
            self.height = height;
        }

        pub fn render<F: FnOnce(&Canvas, u32, u32)>(&mut self, draw: F) {
            let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;

            {
                let surface = &mut self.surfaces[index];
                draw(surface.canvas(), self.width, self.height);
            }
            // Flush + transition the buffer for present.
            let surface = &mut self.surfaces[index];
            self.gr_context.flush_and_submit_surface(surface, None);

            // NOTE: under DirectComposition, presentation is always composited
            // by DWM at the display refresh — there's no uncapped/tearing path
            // like a WGL swapchain had. `vsync=false` still presents every
            // frame; it just can't exceed the refresh rate. The 500fps-OLED
            // target therefore means "present every 2ms vblank", not "tear".
            let sync = if self.vsync.get() { 1 } else { 0 };
            let _ = unsafe { self.swap_chain.Present(sync, DXGI_PRESENT::default()) };

            // LEAK_HUNT_INSTRUMENT — strip before release.
            self.frames = self.frames.wrapping_add(1);
            if self.frames.is_multiple_of(300) {
                self.gr_context
                    .perform_deferred_cleanup(std::time::Duration::from_secs(3), None);
            }
            if self.frames.is_multiple_of(3600) {
                let usage = self.gr_context.resource_cache_usage();
                let limit = self.gr_context.resource_cache_limit();
                log::info!(
                    "skia gpu: cache {} resources, {:.1}/{:.0} MB",
                    usage.resource_count,
                    usage.resource_bytes as f64 / (1024.0 * 1024.0),
                    limit as f64 / (1024.0 * 1024.0),
                );
                super::log_skia_global_cache_state();
            }
            // ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────
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
    ) -> Vec<SkSurface> {
        (0..BUFFER_COUNT)
            .map(|i| {
                let resource = swap_chain.GetBuffer(i).expect("swapchain GetBuffer");
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
                .expect("wrap_backend_render_target (d3d)")
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

        /// LEAK_HUNT_INSTRUMENT — strip before release.
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

            // LEAK_HUNT_INSTRUMENT — strip before release.
            gr_context.set_resource_cache_limit(192 * 1024 * 1024);
            // ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────

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

            // LEAK_HUNT_INSTRUMENT — strip before release.
            self.frames = self.frames.wrapping_add(1);
            if self.frames.is_multiple_of(300) {
                self.gr_context
                    .perform_deferred_cleanup(std::time::Duration::from_secs(3), None);
            }
            if self.frames.is_multiple_of(3600) {
                let usage = self.gr_context.resource_cache_usage();
                let limit = self.gr_context.resource_cache_limit();
                log::info!(
                    "skia gpu: cache {} resources, {:.1}/{:.0} MB",
                    usage.resource_count,
                    usage.resource_bytes as f64 / (1024.0 * 1024.0),
                    limit as f64 / (1024.0 * 1024.0),
                );
                super::log_skia_global_cache_state();
            }
            // ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────
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
