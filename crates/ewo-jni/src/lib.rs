//! `ewo-jni` — Phase E HUD bridge.
//!
//! A `cdylib` loaded into the Minecraft JVM by the `ewo-hud` Fabric mod. It
//! paints `ewo-render`'s Skia UI over Minecraft's framebuffer.
//!
//! **GL context isolation.** Skia and Minecraft each drive the OpenGL state
//! machine and neither tolerates the other mutating it. Sharing one context
//! corrupts state both ways — visible as flickering UI, fatal as a driver
//! crash. So this bridge creates a **dedicated GL context** on Minecraft's
//! window: each frame it `wglMakeCurrent`s to its own context, draws, and
//! hands the thread's context back to Minecraft untouched. Two separate state
//! machines, one shared window framebuffer.
//!
//! **Two clocks (E1).** The HUD is *painted* and *composited* as two separate
//! steps, decoupled so a user can cap the expensive paint without the HUD
//! tearing (constraint #5):
//!
//!   - `paint(t)` renders the whole HUD to an **offscreen GPU surface**. It is
//!     rate-gated: if less than `1 / hud_paint_rate` seconds have elapsed since
//!     the last paint it is skipped, and the offscreen surface keeps its prior
//!     contents.
//!   - `composite()` blits that offscreen surface onto `fbo 0`. It runs
//!     **every** frame, so the HUD never tears or vanishes between paints.
//!
//! `hud_paint_rate` defaults to `Match` (paint every frame); it's chosen in the
//! in-game settings overlay and persisted in `hud.toml`.
//!
//! **Tradeoff.** Painting to an offscreen surface means glass panels no longer
//! backdrop-blur the *live* game (the draw-direct spike got that for free).
//! Fine for small HUD widgets; revisited for the settings overlay in E7.
//!
//! **Widgets + data (E2–E3).** HUD content lives in [`hud`]. The mod allocates
//! a direct `ByteBuffer`, hands it to `nativeInit` once, then fills it each
//! frame; Rust reads it through the buffer's address with no per-frame JNI
//! marshaling. The full read-only widget set ships in E3.
//!
//! **Overlay input + editor (E4–E5).** A keybind opens a custom Minecraft
//! `Screen` that frees the cursor and forwards mouse input to Rust; while
//! closed the HUD is display-only and the game owns all input. E5's HUD
//! editor consumes that input — widgets are placed from a persisted
//! `hud.toml` layout and can be dragged to reposition them while the overlay
//! is open.
//!
//! JNI contract — must match `dev.lewlone.ewohud.EwoHudNative`:
//! ```text
//!   static native void nativeHello();              // bridge liveness check
//!   static native void nativeInit(ByteBuffer buf); // register the shared data block
//!   static native void nativeRender();             // paint + composite one frame
//!   static native void nativeMouseMove / nativeMouseButton / nativeMouseScroll
//!   static native void nativeKey                   // overlay input (E4)
//! ```
//! All are invoked on Minecraft's render thread.

#![allow(non_snake_case)] // JNI exports must be named `Java_<pkg>_<class>_<method>`.

mod hud;

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Once, OnceLock};
use std::time::Instant;

use ewo_render::FontStore;
use skia_safe::gpu::gl::{Format, FramebufferInfo, Interface};
use skia_safe::gpu::{
    backend_render_targets, direct_contexts, surfaces, Budgeted, DirectContext, SurfaceOrigin,
};
use skia_safe::{AlphaType, Color, ColorType, ImageInfo, Surface};

// ────────────────────────────────────────────────────────────────────────
// Win32 / WGL
// ────────────────────────────────────────────────────────────────────────

#[link(name = "opengl32")]
extern "system" {
    fn wglGetProcAddress(name: *const u8) -> *const c_void;
    fn wglGetCurrentContext() -> *mut c_void;
    fn wglGetCurrentDC() -> *mut c_void;
    fn wglCreateContext(hdc: *mut c_void) -> *mut c_void;
    fn wglMakeCurrent(hdc: *mut c_void, hglrc: *mut c_void) -> i32;
    fn wglDeleteContext(hglrc: *mut c_void) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}
#[link(name = "user32")]
extern "system" {
    fn WindowFromDC(hdc: *mut c_void) -> *mut c_void;
    fn GetClientRect(hwnd: *mut c_void, rect: *mut WinRect) -> i32;
}

#[repr(C)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Resolve a GL entry point: `wglGetProcAddress` for modern functions,
/// `opengl32.dll` exports for the GL 1.1 core it won't return.
fn gl_get_proc(name: &str) -> *const c_void {
    let Ok(cname) = CString::new(name) else {
        return ptr::null();
    };
    unsafe {
        let p = wglGetProcAddress(cname.as_ptr() as *const u8);
        let v = p as isize;
        if !p.is_null() && v != 1 && v != 2 && v != 3 && v != -1 {
            return p;
        }
        let module = GetModuleHandleA(b"opengl32.dll\0".as_ptr());
        if module.is_null() {
            return ptr::null();
        }
        GetProcAddress(module, cname.as_ptr() as *const u8)
    }
}

// ────────────────────────────────────────────────────────────────────────
// Logging — stderr (captured into the launcher's per-launch log) plus a
// standalone file at %TEMP%\ewo-jni.log.
// ────────────────────────────────────────────────────────────────────────

static LOG_INIT: Once = Once::new();

fn log(msg: &str) {
    eprintln!("[ewo-jni] {msg}");
    let path = std::env::temp_dir().join("ewo-jni.log");
    LOG_INIT.call_once(|| {
        let _ = std::fs::write(&path, b"");
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[ewo-jni] {msg}");
    }
}

// ────────────────────────────────────────────────────────────────────────
// Paint rate — the user-facing cap on the expensive paint step (constraint #5).
// ────────────────────────────────────────────────────────────────────────

/// How often the HUD is repainted to its offscreen surface. The composite
/// step always runs every frame regardless — this only gates `paint`. Chosen
/// in the in-game settings overlay and persisted in `hud.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HudPaintRate {
    /// Repaint every frame (default — matches the launcher's identity).
    Match,
    Fps120,
    Fps60,
    Fps30,
}

impl HudPaintRate {
    /// Every rate, in settings-selector order.
    pub(crate) const ALL: [HudPaintRate; 4] = [
        HudPaintRate::Match,
        HudPaintRate::Fps120,
        HudPaintRate::Fps60,
        HudPaintRate::Fps30,
    ];

    /// Minimum wall-clock seconds between paints. `Match` → 0 (never gates).
    pub(crate) fn min_interval(self) -> f32 {
        match self {
            HudPaintRate::Match => 0.0,
            HudPaintRate::Fps120 => 1.0 / 120.0,
            HudPaintRate::Fps60 => 1.0 / 60.0,
            HudPaintRate::Fps30 => 1.0 / 30.0,
        }
    }

    /// Short label for the settings selector.
    pub(crate) fn label(self) -> &'static str {
        match self {
            HudPaintRate::Match => "MATCH",
            HudPaintRate::Fps120 => "120",
            HudPaintRate::Fps60 => "60",
            HudPaintRate::Fps30 => "30",
        }
    }

    /// Token for `hud.toml`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HudPaintRate::Match => "match",
            HudPaintRate::Fps120 => "120",
            HudPaintRate::Fps60 => "60",
            HudPaintRate::Fps30 => "30",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<HudPaintRate> {
        Some(match s {
            "match" => HudPaintRate::Match,
            "120" => HudPaintRate::Fps120,
            "60" => HudPaintRate::Fps60,
            "30" => HudPaintRate::Fps30,
            _ => return None,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────
// HUD state — one per render thread (Skia's DirectContext is not Send).
// ────────────────────────────────────────────────────────────────────────

static GL_LOADED: Once = Once::new();
static START: OnceLock<Instant> = OnceLock::new();
/// Address of the shared JVM→Rust data block, registered by `nativeInit`.
/// `0` until then. Set on the render thread before the first `nativeRender`.
static HUD_BUFFER: AtomicUsize = AtomicUsize::new(0);
/// Logs a buffer-schema mismatch at most once.
static SCHEMA_WARN: Once = Once::new();

fn elapsed_secs() -> f32 {
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

enum HudState {
    Uninit,
    Failed,
    Ready(Hud),
}

thread_local! {
    static HUD: RefCell<HudState> = const { RefCell::new(HudState::Uninit) };
}

struct Hud {
    /// Minecraft's window device context — shared by both GL contexts.
    hdc: *mut c_void,
    /// Minecraft's GL context — borrowed for reference, never made current by us
    /// except to hand the thread back to it.
    mc_ctx: *mut c_void,
    /// Our dedicated GL context. Skia owns its state machine entirely.
    our_ctx: *mut c_void,
    gr: DirectContext,
    /// Offscreen GPU surface the HUD is painted to (window-sized). This is the
    /// "cache" of the two-clock model: `composite` reads it every frame, even
    /// on frames where `paint` was rate-gated and skipped.
    offscreen: Option<Surface>,
    /// Pixel size `offscreen` was created at — recreated when the window resizes.
    offscreen_size: (i32, i32),
    /// Wall-clock seconds of the last completed paint. `NEG_INFINITY` until the
    /// first paint so the gate always lets frame one through.
    last_painted: f32,
    composites: u64,
    paints: u64,
    /// Variable fonts (Fraunces / JetBrains Mono / …) for the HUD widgets.
    font_store: FontStore,
    /// Address of the shared data block, refreshed from `HUD_BUFFER` each frame.
    buffer: usize,
    /// HUD-editor state (layout, drag), fed by the `nativeMouse*` exports.
    editor: hud::Editor,
}

// `Hud` lives in a thread-local for the process lifetime; it is never dropped
// before exit, so no `Drop`/`wglDeleteContext` cleanup is wired up.

impl Hud {
    /// Create a dedicated GL context on Minecraft's window and build a Skia
    /// `DirectContext` against it. Called once, on the render thread, with
    /// Minecraft's context current.
    fn create() -> Option<Hud> {
        let hdc = unsafe { wglGetCurrentDC() };
        let mc_ctx = unsafe { wglGetCurrentContext() };
        if hdc.is_null() || mc_ctx.is_null() {
            log("no current WGL context — Minecraft's GL context not found");
            return None;
        }

        let our_ctx = unsafe { wglCreateContext(hdc) };
        if our_ctx.is_null() {
            log("wglCreateContext failed");
            return None;
        }
        if unsafe { wglMakeCurrent(hdc, our_ctx) } == 0 {
            log("wglMakeCurrent(dedicated context) failed");
            unsafe { wglDeleteContext(our_ctx) };
            return None;
        }

        GL_LOADED.call_once(|| {
            gl::load_with(gl_get_proc);
            log("GL function pointers loaded via wglGetProcAddress");
        });

        let gr = Interface::new_load_with(|name| {
            if name.starts_with("egl") {
                return ptr::null();
            }
            gl_get_proc(name)
        })
        .and_then(|interface| direct_contexts::make_gl(interface, None));

        // Hand the thread's context back to Minecraft, success or not.
        unsafe { wglMakeCurrent(hdc, mc_ctx) };

        let Some(gr) = gr else {
            unsafe { wglDeleteContext(our_ctx) };
            log("Skia DirectContext creation failed");
            return None;
        };

        // CPU-side font loading — no GL context needed. Resolves the workspace
        // `assets/fonts/` dir via the compile-time path baked into `ewo-render`.
        let font_store = FontStore::new();
        log(&format!(
            "fonts loaded: fraunces={} jetbrains_mono={}",
            font_store.has_fraunces, font_store.has_jetbrains_mono
        ));

        log("Skia DirectContext created on a dedicated GL context, isolated from Minecraft");
        Some(Hud {
            hdc,
            mc_ctx,
            our_ctx,
            gr,
            offscreen: None,
            offscreen_size: (0, 0),
            last_painted: f32::NEG_INFINITY,
            composites: 0,
            paints: 0,
            font_store,
            buffer: 0,
            editor: hud::Editor::new(),
        })
    }

    /// One frame: refresh the data-block address, paint (rate-gated) the HUD to
    /// the offscreen surface, then composite it onto Minecraft's framebuffer.
    /// Runs on our dedicated context; Minecraft's context is handed back
    /// untouched at the end.
    fn frame(&mut self, buffer: usize) {
        self.buffer = buffer;

        let Some((w, h)) = self.window_size() else {
            return;
        };

        // From here until we hand it back, the thread runs on our context —
        // Minecraft's GL state is untouchable.
        if unsafe { wglMakeCurrent(self.hdc, self.our_ctx) } == 0 {
            log("wglMakeCurrent(dedicated context) failed in frame");
            return;
        }

        // Minecraft mutated GL state since we last ran — resync Skia's view.
        self.gr.reset(None);
        unsafe { gl::Viewport(0, 0, w, h) };

        self.ensure_offscreen(w, h);
        self.paint(elapsed_secs(), w, h);
        self.composite(w, h);

        // Hand the thread's context back to Minecraft.
        unsafe { wglMakeCurrent(self.hdc, self.mc_ctx) };
    }

    /// Create (or recreate, on window resize) the offscreen GPU surface the
    /// HUD is painted to. No-op when one of the right size already exists.
    fn ensure_offscreen(&mut self, w: i32, h: i32) {
        if self.offscreen.is_some() && self.offscreen_size == (w, h) {
            return;
        }
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Premul, None);
        self.offscreen = surfaces::render_target(
            &mut self.gr,
            Budgeted::Yes,
            &info,
            None,                   // sample_count — no MSAA; Skia AAs the glyphs itself
            SurfaceOrigin::TopLeft, // logical orientation; composite handles the blit
            None,                   // surface_props
            false,                  // mipmaps
            false,                  // protected
        );
        match &self.offscreen {
            Some(_) => {
                self.offscreen_size = (w, h);
                log(&format!("offscreen HUD surface created ({w}x{h})"));
            }
            None => {
                self.offscreen_size = (0, 0);
                log("offscreen HUD surface creation failed");
            }
        }
    }

    /// Paint clock. Render the HUD onto the offscreen surface — but only if at
    /// least `1 / paint_rate` seconds have passed since the last paint. When
    /// gated out, the offscreen surface keeps its prior contents and
    /// `composite` simply re-blits the stale image.
    fn paint(&mut self, now: f32, w: i32, h: i32) {
        if now - self.last_painted < self.editor.paint_rate().min_interval() {
            return; // capped — composite reuses the offscreen surface as-is
        }
        let Some(surface) = self.offscreen.as_mut() else {
            return;
        };
        let canvas = surface.canvas();
        // Clear to transparent so only the widgets composite over the game.
        canvas.clear(Color::TRANSPARENT);

        if self.buffer != 0 {
            // SAFETY: `buffer` is the address of the mod's direct `ByteBuffer`,
            // held for the process lifetime (`EwoHudData.CAPACITY` bytes).
            let data = unsafe { hud::HudData::new(self.buffer as *const u8) };
            if data.schema_version() == hud::SCHEMA_VERSION {
                hud::draw(canvas, &data, &mut self.editor, &self.font_store, w as f32, h as f32);
            } else {
                SCHEMA_WARN.call_once(|| {
                    log(&format!(
                        "HUD data block schema {} != expected {} — widgets disabled",
                        data.schema_version(),
                        hud::SCHEMA_VERSION
                    ));
                });
            }
        }

        self.last_painted = now;
        self.paints += 1;
    }

    /// Composite clock. Blit the offscreen surface onto Minecraft's
    /// framebuffer (`fbo 0`). Runs every frame so the HUD is always shown,
    /// even on frames where `paint` was rate-gated.
    fn composite(&mut self, w: i32, h: i32) {
        let fb_info = FramebufferInfo {
            fboid: 0, // the window's default framebuffer, shared across both contexts
            format: Format::RGBA8.into(),
            ..Default::default()
        };
        let rt = backend_render_targets::make_gl((w, h), 0, 0, fb_info);
        let mut fbo = match surfaces::wrap_backend_render_target(
            &mut self.gr,
            &rt,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            None,
            None,
        ) {
            Some(s) => s,
            None => {
                log("composite: wrap_backend_render_target returned None");
                return;
            }
        };

        if let Some(offscreen) = self.offscreen.as_mut() {
            // `image_snapshot` is copy-on-write: this snapshot is dropped at the
            // end of the frame, so the next `paint` renders into the offscreen
            // surface in place (no texture copy).
            let image = offscreen.image_snapshot();
            fbo.canvas().draw_image(&image, (0.0, 0.0), None);
        }

        self.gr.flush_and_submit();

        self.composites += 1;
        if self.composites == 1 {
            log(&format!(
                "first composite ({w}x{h}) — two-clock HUD compositing over Minecraft"
            ));
        }
        if self.composites % 600 == 0 {
            // In `Match` mode paints == composites; a capped rate shows paints
            // lagging well behind — the proof the two-clock cap is working.
            log(&format!(
                "{} composites, {} paints (rate {:?})",
                self.composites,
                self.paints,
                self.editor.paint_rate()
            ));
        }
    }

    /// Window client size in pixels — straight from Win32, independent of GL
    /// state, so it works regardless of which context is current.
    fn window_size(&self) -> Option<(i32, i32)> {
        unsafe {
            let hwnd = WindowFromDC(self.hdc);
            if hwnd.is_null() {
                return None;
            }
            let mut r = WinRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetClientRect(hwnd, &mut r) == 0 {
                return None;
            }
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w > 0 && h > 0 {
                Some((w, h))
            } else {
                None
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// JNI exports
// ────────────────────────────────────────────────────────────────────────

/// Run `f` against the render thread's `Hud` if it has been created. Used by
/// the overlay-input exports — input only matters once the HUD is up.
fn with_hud(f: impl FnOnce(&mut Hud)) {
    let _ = panic::catch_unwind(AssertUnwindSafe(move || {
        HUD.with(|cell| {
            if let HudState::Ready(hud) = &mut *cell.borrow_mut() {
                f(hud);
            }
        });
    }));
}

/// Liveness check. Proves the cdylib loaded and JNI linkage works.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeHello(
    _env: *mut c_void,
    _class: *mut c_void,
) {
    let _ = panic::catch_unwind(|| {
        log("nativeHello — JNI bridge alive, Rust side responding");
    });
}

/// Register the shared JVM→Rust data block. Called once at mod init with a
/// direct `ByteBuffer`; Rust resolves its address and reads it every frame
/// thereafter with no further JNI marshaling.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeInit(
    env: *mut jni_sys::JNIEnv,
    _class: *mut c_void,
    buf: jni_sys::jobject,
) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if env.is_null() || buf.is_null() {
            log("nativeInit: null env or buffer");
            return;
        }
        // SAFETY: `env` is a valid JNIEnv for the calling thread; reading its
        // function table and calling `GetDirectBufferAddress` is the documented
        // JNI contract.
        let addr = unsafe {
            match (**env).GetDirectBufferAddress {
                Some(get_addr) => get_addr(env, buf),
                None => {
                    log("nativeInit: GetDirectBufferAddress unavailable");
                    return;
                }
            }
        };
        if addr.is_null() {
            log("nativeInit: GetDirectBufferAddress returned null (buffer not direct?)");
            return;
        }
        HUD_BUFFER.store(addr as usize, Ordering::Relaxed);
        log("nativeInit: HUD data block registered");
    }));
}

/// Paint + composite one HUD frame, reading live data from the shared block.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeRender(
    _env: *mut c_void,
    _class: *mut c_void,
) {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        HUD.with(|cell| {
            let mut state = cell.borrow_mut();
            if matches!(*state, HudState::Uninit) {
                *state = match Hud::create() {
                    Some(hud) => HudState::Ready(hud),
                    None => {
                        log("Skia init failed — HUD disabled for this thread");
                        HudState::Failed
                    }
                };
            }
            if let HudState::Ready(hud) = &mut *state {
                hud.frame(HUD_BUFFER.load(Ordering::Relaxed));
            }
        });
    }));
    if result.is_err() {
        log("nativeRender panicked (caught at JNI boundary)");
    }
}

/// Overlay input — cursor moved to `(x, y)` in window pixels.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeMouseMove(
    _env: *mut c_void,
    _class: *mut c_void,
    x: f64,
    y: f64,
) {
    with_hud(|hud| hud.editor.on_mouse_move(x as f32, y as f32));
}

/// Overlay input — a mouse button pressed/released at `(x, y)` in window
/// pixels. Drives the HUD editor's drag.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeMouseButton(
    _env: *mut c_void,
    _class: *mut c_void,
    _button: i32,
    pressed: u8,
    x: f64,
    y: f64,
) {
    with_hud(|hud| hud.editor.on_mouse_button(pressed != 0, x as f32, y as f32));
}

/// Overlay input — the scroll wheel. Unused until the E6 settings overlay.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeMouseScroll(
    _env: *mut c_void,
    _class: *mut c_void,
    _dx: f64,
    _dy: f64,
) {
}

/// Overlay input — a key. Overlay open/close is handled Java-side; this is
/// unused until the E6 settings overlay needs text/hotkeys.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeKey(
    _env: *mut c_void,
    _class: *mut c_void,
    _key: i32,
    _pressed: u8,
    _modifiers: i32,
) {
}
