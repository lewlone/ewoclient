//! OpenGL backend — Skia on top of glutin's GL context.
//!
//! Build-sequence step 2. Replaces `softbuffer` from step 1.
//!
//! Lifetime: created in `ApplicationHandler::resumed()` after the winit Window
//! exists. `resize()` updates both the GL surface and the wrapped Skia surface.
//! `render(|canvas| { ... })` makes the context current, builds a fresh
//! `SkSurface` over the default framebuffer, hands the closure a `&Canvas`,
//! flushes, and swaps buffers.

use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;

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
    /// Frame counter — drives the periodic GPU-resource cleanup + diagnostic
    /// log. Wraps at ~136 years @ 1000fps; fine.
    frames: u64,
}

impl GlBackend {
    pub fn new(event_loop: &ActiveEventLoop, window: Arc<Window>) -> Self {
        let (gl_display, gl_config) = {
            let template = ConfigTemplateBuilder::new()
                .with_alpha_size(8)
                .with_stencil_size(8);

            // glutin-winit DisplayBuilder. We pass `None` for window attrs
            // because we already created the window. It just builds the
            // display from the active event loop's raw display handle.
            let display_builder = DisplayBuilder::new();
            let (_w, gl_config) = display_builder
                .build(event_loop, template, |configs| {
                    // Pick the config with the highest sample count.
                    configs
                        .reduce(|acc, c| if c.num_samples() > acc.num_samples() { c } else { acc })
                        .expect("no GL config")
                })
                .expect("DisplayBuilder::build failed");
            let gl_display = gl_config.display();
            (gl_display, gl_config)
        };

        let raw_window_handle = window
            .window_handle()
            .expect("window_handle")
            .as_raw();

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

        // VSync starts on; toggle at runtime via `GlBackend::set_vsync(false)`
        // for uncapped frame-rate validation (500fps OLED target).
        let _ = gl_surface.set_swap_interval(
            &gl_context,
            SwapInterval::Wait(NonZeroU32::new(1).unwrap()),
        );

        // Load GL function pointers for the `gl` crate (used to query the
        // current framebuffer binding) and for Skia.
        gl::load_with(|s| {
            let cstr = CString::new(s).unwrap();
            gl_display.get_proc_address(&cstr) as *const _
        });

        let interface = Interface::new_load_with(|name| {
            // Skia probes for some symbols that glutin won't have on every
            // platform. Returning null for those is fine.
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

        let mut gr_context = direct_contexts::make_gl(interface, None)
            .expect("direct_contexts::make_gl");

        // LEAK_HUNT_INSTRUMENT — strip before release.
        // Cap Skia's GPU resource cache at 192 MB (default 256 MB). Was
        // added during leak-hunt; harmless but not actually fixing
        // anything since the real leak was outside Skia's tracked caches.
        gr_context.set_resource_cache_limit(192 * 1024 * 1024);
        // ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────────

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
            log_skia_global_cache_state();
        }
        // ── end LEAK_HUNT_INSTRUMENT ──────────────────────────────────────
    }

    /// Toggle vsync on or off. `true` waits for the display refresh
    /// (capped to monitor rate, no tearing); `false` swaps without waiting
    /// (uncapped frame-rate, used to validate the 500fps target on OLED
    /// hardware).
    ///
    /// On platforms where the requested swap interval is unsupported the
    /// call silently fails — the previous interval persists.
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
