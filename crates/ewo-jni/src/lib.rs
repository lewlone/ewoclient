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
//! **Glass refract (E7).** Painting to an offscreen surface means HUD widgets
//! can't backdrop-blur the *live* game. E7's call: the MODS/SETTINGS overlay
//! views frost the whole game so the overlay reads as glass over depth; the
//! HUD editor leaves the game sharp so widgets stay readable against it while
//! being positioned. The frost is a genuine blur but it is *cached* — a wide
//! gaussian is expensive and the game behind an open overlay barely changes,
//! so it gets its own slow clock (a third one alongside paint and composite):
//! `refresh_frost` recomputes the blur only a few times per second into a
//! quarter-resolution surface, and `composite` upscales that cached surface
//! every frame for the price of one textured quad. The blur itself is a clean
//! two-step 2× downscale (each linear 2× step averages an exact 2×2 block, so
//! it never aliases) + a small gaussian + a cubic upscale — that reads as a
//! smooth wide gaussian, not the block artifacts of one big full-res kernel.
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

// These are `pub` only so `examples/hudshot.rs` can drive the overlay through
// the `rlib` target (see Cargo.toml). The JVM loads the cdylib and reaches the
// crate exclusively through the `Java_…` exports below — nothing here is a
// stable API for anyone else.
pub mod audio;
pub mod crosshair;
pub mod fixture;
pub mod hud;
pub mod media;
pub mod modules;
mod perf;
pub mod pvp;
mod skin;
mod social;

use perf::{Mode, Perf, Sec};

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Once, OnceLock};
use std::time::Instant;

use ewo_render::FontStore;
use skia_safe::gpu::gl::{Format, FramebufferInfo, Interface, TextureInfo};
use skia_safe::gpu::{
    backend_render_targets, backend_textures, direct_contexts, surfaces, Budgeted, DirectContext,
    Mipmapped, Protected, SurfaceOrigin,
};
use skia_safe::image_filters;
use skia_safe::{
    canvas::SrcRectConstraint, AlphaType, ClipOp, Color, ColorType, CubicResampler, FilterMode,
    Image, ImageInfo, Paint, RRect, Rect, Surface, TileMode,
};

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
    /// Share the GL *object* namespace (textures, buffers, programs) between two
    /// contexts. Does **not** share GL *state* — the state isolation that keeps
    /// Skia and Minecraft from corrupting each other is unchanged. `hglrc2` must
    /// have created no objects yet.
    fn wglShareLists(hglrc1: *mut c_void, hglrc2: *mut c_void) -> i32;
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
// `pub` to match `Editor::paint_rate`, which returns it — see the module
// visibility note above the `mod` declarations.
pub enum HudPaintRate {
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
/// Address of the Rust→JVM module-state block, registered by `nativeInitModules`.
/// `0` until then. Rust writes it every frame; the mod reads it. See [`modules`].
static MODULE_BUFFER: AtomicUsize = AtomicUsize::new(0);
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
    /// Cached frosted backdrop — a quarter-resolution blur of the live game,
    /// recomputed only a few times a second and re-blitted cheaply every
    /// composite. `None` until the first frost or after a window resize.
    frost: Option<Surface>,
    /// Half-resolution scratch surface — the clean intermediate of the
    /// 2×→2× downscale that feeds `frost` (one linear 2× step never aliases).
    frost_half: Option<Surface>,
    /// Full window size `frost`/`frost_half` were sized for.
    frost_size: (i32, i32),
    /// Wall-clock seconds of the last frost recompute. `NEG_INFINITY` when the
    /// cache is cold so the next overlay-open frosts immediately.
    last_frosted: f32,
    /// Opt-in render-thread profiler (gated by a `%TEMP%/ewo-perf.on` sentinel;
    /// zero per-frame cost when off). See [`perf`].
    perf: Perf,

    // ── Liquid-glass backdrop capture ────────────────────────────────────
    /// Shared GL texture holding a copy of the live game framebuffer, taken
    /// during composite *before* the HUD is drawn — so the glass refracts the
    /// world and not its own previous frame. Capturing at paint time instead
    /// would feed the HUD back into itself.
    game_tex: u32,
    /// Skia view over [`Self::game_tex`]. Wrapped `BottomLeft` because
    /// `glCopyTexSubImage2D` from the default framebuffer lands bottom-up —
    /// the same reason the composite shader flips V.
    game_surface: Option<Surface>,
    /// The captured world, when the *slow* composite path took it.
    ///
    /// The two paths capture differently and both are correct for their
    /// context: the fast path composites in Minecraft's GL context, where Skia
    /// state must not be touched, so it does a raw `glCopyTexSubImage2D` into
    /// the shared texture and lets `glass_sources` snapshot it later. The slow
    /// path is already inside Skia with fbo 0 wrapped, so a raw GL copy there
    /// would desync Skia's cached GL state — it snapshots through Skia instead
    /// and parks the result here.
    last_game: Option<Image>,
    /// The two blur levels liquid glass samples: half-res lightly blurred (what
    /// the refracting rim bends) and quarter-res heavily blurred (what shows
    /// through the frosted interior).
    glass_rim: Option<Surface>,
    glass_frost: Option<Surface>,
    /// Window size the glass surfaces were built for.
    glass_size: (i32, i32),
    /// Wall-clock of the last framebuffer capture. Capture runs at the paint
    /// cadence, not every frame — the glass is only re-read when it is redrawn.
    last_captured: f32,

    // ── MC-context composite (the per-frame context-switch elimination) ──
    /// `wglShareLists` succeeded — our GL objects are visible to Minecraft's
    /// context, so the per-frame composite can run as a quad in *its* context
    /// with no `wglMakeCurrent`. When false we fall back to the legacy
    /// switch-every-frame Skia composite.
    shared: bool,
    /// GL id of the texture the HUD is painted into. We own it (so the id is
    /// stable and shareable) and wrap it as `offscreen` for Skia. `0` until the
    /// first `ensure_offscreen`. Sampled by `composite_mc` in Minecraft's context.
    hud_tex: u32,
    /// Shader program that draws `hud_tex` as a full-screen quad. A *shared*
    /// object (built in our context, used from Minecraft's). `0` if it failed
    /// to build — then `shared` is forced false.
    comp_program: u32,
    /// `u_tex` sampler uniform location in `comp_program`.
    comp_tex_loc: i32,
    /// `u_solid` diagnostic-uniform location (fixed-colour test mode).
    comp_solid_loc: i32,
    /// Vertex array object for the composite draw. VAOs are **not** shared, so
    /// this is created lazily in Minecraft's context on the first `composite_mc`.
    comp_vao: u32,
    /// Sampler object bound for the composite draw (NEAREST, no mips, clamp). It
    /// overrides whatever sampler params Skia left on `hud_tex`, guaranteeing the
    /// texture is *complete* when Minecraft's context samples it — otherwise GL
    /// returns `(0,0,0,1)` (opaque black). Created lazily with `comp_vao`.
    comp_sampler: u32,
}

/// The frosted backdrop is recomputed at most this often. The game behind an
/// open overlay barely changes, so a low rate is visually invisible and keeps
/// the expensive blur off the per-frame composite path.
const FROST_REFRESH_INTERVAL: f32 = 1.0 / 10.0;

/// Allocate a budgeted offscreen GPU surface, `w`×`h`, RGBA8 premul, no MSAA
/// (Skia anti-aliases glyphs itself). `TopLeft` origin — callers blit it.
fn gpu_surface(gr: &mut DirectContext, w: i32, h: i32) -> Option<Surface> {
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Premul, None);
    surfaces::render_target(
        gr,
        Budgeted::Yes,
        &info,
        None,                   // sample_count
        SurfaceOrigin::TopLeft, // logical orientation
        None,                   // surface_props
        false,                  // mipmaps
        false,                  // protected
    )
}

/// Compile + link the full-screen composite shader. Returns `(program, u_tex
/// uniform location)`. Built while *our* context is current (init); the program
/// is a shared GL object so Minecraft's context uses it every frame.
///
/// The vertex stage is a `gl_VertexID` full-screen triangle — no VBO or vertex
/// attributes. The fragment stage samples the HUD texture, flipping V because
/// Skia paints with a top-left origin; premultiplied-alpha blending is set by
/// the caller (`ONE, ONE_MINUS_SRC_ALPHA`).
unsafe fn build_composite_program() -> Option<(u32, i32, i32)> {
    let vs = b"#version 330 core\n\
        out vec2 v_uv;\n\
        void main(){\n\
        vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));\n\
        v_uv = p;\n\
        gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);\n\
        }\0";
    // `u_solid` is a diagnostic: when set it ignores the texture and outputs a
    // fixed semi-transparent colour, so a black-screen bug can be split into
    // "blend/state path" vs "texture-sampling path" without a rebuild.
    let fs = b"#version 330 core\n\
        in vec2 v_uv;\n\
        uniform sampler2D u_tex;\n\
        uniform int u_solid;\n\
        out vec4 frag;\n\
        void main(){\n\
        if (u_solid != 0) frag = vec4(0.0, 0.55, 0.0, 0.5);\n\
        else frag = texture(u_tex, vec2(v_uv.x, 1.0 - v_uv.y));\n\
        }\0";

    let v = compile_shader(gl::VERTEX_SHADER, vs)?;
    let f = compile_shader(gl::FRAGMENT_SHADER, fs)?;
    let prog = gl::CreateProgram();
    gl::AttachShader(prog, v);
    gl::AttachShader(prog, f);
    gl::LinkProgram(prog);
    gl::DeleteShader(v);
    gl::DeleteShader(f);
    let mut ok: i32 = 0;
    gl::GetProgramiv(prog, gl::LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 512];
        let mut len = 0i32;
        gl::GetProgramInfoLog(prog, buf.len() as i32, &mut len, buf.as_mut_ptr() as *mut i8);
        log(&format!(
            "composite program link failed: {}",
            String::from_utf8_lossy(&buf[..len.max(0) as usize])
        ));
        gl::DeleteProgram(prog);
        return None;
    }
    let tex = gl::GetUniformLocation(prog, b"u_tex\0".as_ptr() as *const i8);
    let solid = gl::GetUniformLocation(prog, b"u_solid\0".as_ptr() as *const i8);
    Some((prog, tex, solid))
}

unsafe fn compile_shader(kind: u32, src: &[u8]) -> Option<u32> {
    let sh = gl::CreateShader(kind);
    let ptr = src.as_ptr() as *const i8;
    gl::ShaderSource(sh, 1, &ptr, ptr::null()); // src is NUL-terminated
    gl::CompileShader(sh);
    let mut ok: i32 = 0;
    gl::GetShaderiv(sh, gl::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 512];
        let mut len = 0i32;
        gl::GetShaderInfoLog(sh, buf.len() as i32, &mut len, buf.as_mut_ptr() as *mut i8);
        log(&format!(
            "composite shader compile failed: {}",
            String::from_utf8_lossy(&buf[..len.max(0) as usize])
        ));
        gl::DeleteShader(sh);
        return None;
    }
    Some(sh)
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
        // Share the GL object namespace so the HUD texture Skia creates in our
        // context can be sampled from Minecraft's context for the composite quad.
        // Must precede any object creation in our context (Skia, below). Shares
        // objects only, never GL state — the isolation model is unchanged.
        let shared = unsafe { wglShareLists(mc_ctx, our_ctx) } != 0;
        if !shared {
            log("wglShareLists failed — composite falls back to per-frame context switch");
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

        // Build the composite shader program while our context is current — it's
        // a *shared* object, so Minecraft's context can use it every frame.
        let (comp_program, comp_tex_loc, comp_solid_loc) =
            unsafe { build_composite_program() }.unwrap_or((0, -1, -1));

        // Hand the thread's context back to Minecraft, success or not.
        unsafe { wglMakeCurrent(hdc, mc_ctx) };

        let Some(gr) = gr else {
            unsafe { wglDeleteContext(our_ctx) };
            log("Skia DirectContext creation failed");
            return None;
        };

        // The fast (MC-context) composite needs both the shared namespace and a
        // working shader. Either missing → fall back to the legacy switch path.
        // A `%TEMP%/ewo-no-mc-composite` sentinel forces the legacy path too —
        // a no-rebuild kill switch if the new path misbehaves on a given driver.
        let force_legacy = std::env::temp_dir().join("ewo-no-mc-composite").exists();
        if force_legacy {
            log("MC-context composite disabled by ewo-no-mc-composite sentinel — legacy path");
        }
        let shared = shared && comp_program != 0 && !force_legacy;

        // CPU-side font loading — no GL context needed. Resolves the workspace
        // `assets/fonts/` dir via the compile-time path baked into `ewo-render`.
        let font_store = FontStore::new();
        log(&format!(
            "fonts loaded: fraunces={} jetbrains_mono={}",
            font_store.has_fraunces, font_store.has_jetbrains_mono
        ));

        let perf = Perf::new();
        log(&format!(
            "Skia DirectContext created on a dedicated GL context, isolated from Minecraft \
             (composite: {}, perf: {})",
            if shared {
                "MC-context quad (no per-frame switch)"
            } else {
                "legacy per-frame switch"
            },
            if perf.enabled {
                if perf.ab_enabled() {
                    "ON + A/B"
                } else {
                    "ON"
                }
            } else {
                "off"
            }
        ));
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
            frost: None,
            frost_half: None,
            frost_size: (0, 0),
            last_frosted: f32::NEG_INFINITY,
            perf,
            shared,
            hud_tex: 0,
            game_tex: 0,
            game_surface: None,
            last_game: None,
            glass_rim: None,
            glass_frost: None,
            glass_size: (0, 0),
            last_captured: f32::NEG_INFINITY,
            comp_program,
            comp_tex_loc,
            comp_solid_loc,
            comp_vao: 0,
            comp_sampler: 0,
        })
    }

    /// One frame: refresh the data-block address, paint (rate-gated) the HUD to
    /// the offscreen surface, then composite it onto Minecraft's framebuffer.
    /// Runs on our dedicated context; Minecraft's context is handed back
    /// untouched at the end.
    fn frame(&mut self, buffer: usize, module_buffer: usize) {
        self.buffer = buffer;

        // The module-state channel — written every frame (it is ~120 bytes,
        // and the mod reads it each frame to drive the effect mixins). It is
        // independent of the window size / paint, so it runs before the
        // early-return below and is never rate-gated.
        if module_buffer != 0 {
            unsafe { self.editor.modules.write_buffer(module_buffer as *mut u8) };
        }

        let Some((w, h)) = self.window_size() else {
            return;
        };

        // Profiler — no-op unless the `%TEMP%/ewo-perf.on` sentinel existed at
        // init. Records the frame period and returns this frame's A/B mode; in
        // `Bypass` (only with the `ewo-perf.ab` sentinel) we skip all GPU work
        // so the HUD's true end-to-end cost is the full-vs-bypass period delta.
        let prof = self.perf.enabled;
        let rate_label = self.editor.paint_rate().as_str();
        let mode = if prof {
            self.perf.begin_frame(w, h, rate_label)
        } else {
            Mode::Full
        };
        if mode == Mode::Bypass {
            return; // HUD blinks off this window; period already recorded
        }
        let injected_t = prof.then(Instant::now);

        // E7 glass refract: the MODS/SETTINGS overlay views frost the live game
        // behind them, which needs Skia to read + blur the live framebuffer —
        // only possible in our context. Those frames (and the fallback when the
        // shared composite is unavailable) take the legacy switch-in/out path.
        let frost = self.buffer != 0
            && unsafe { hud::HudData::new(self.buffer as *const u8) }.overlay_open()
            && self.editor.frosts_game();
        if !frost {
            // Cache goes cold while the frost isn't shown, so the next open
            // recomputes immediately instead of flashing a stale frame.
            self.last_frosted = f32::NEG_INFINITY;
        }

        if frost || !self.shared {
            // ── Legacy path ── full Skia paint + composite in our context, with
            // one `wglMakeCurrent` in and out. Correct but pays the per-frame
            // context-switch cost; reserved for frosted overlay views.
            let t = prof.then(Instant::now);
            if unsafe { wglMakeCurrent(self.hdc, self.our_ctx) } == 0 {
                log("wglMakeCurrent(dedicated context) failed in frame");
                return;
            }
            if let Some(t) = t {
                self.perf.rec(Sec::McTo, t.elapsed().as_nanos() as u64);
            }
            let resized = self.offscreen_size != (w, h);
            if resized {
                let t = prof.then(Instant::now);
                self.gr.reset(None);
                if let Some(t) = t {
                    self.perf.rec(Sec::Reset, t.elapsed().as_nanos() as u64);
                }
            }
            unsafe { gl::Viewport(0, 0, w, h) };
            self.ensure_offscreen(w, h);
            self.ensure_glass_surfaces(w, h);
            let pt = prof.then(Instant::now);
            let painted = self.paint(elapsed_secs(), w, h);
            if let (Some(pt), true) = (pt, painted) {
                self.perf.rec(Sec::Paint, pt.elapsed().as_nanos() as u64);
                self.perf.note_paint();
            }
            self.composite(w, h, frost);
            let t = prof.then(Instant::now);
            unsafe { wglMakeCurrent(self.hdc, self.mc_ctx) };
            if let Some(t) = t {
                self.perf.rec(Sec::McBack, t.elapsed().as_nanos() as u64);
            }
        } else {
            // ── Fast path ── paint (rate-gated) in our context, but composite
            // as a quad in *Minecraft's* context with no per-frame switch. On
            // most frames `should_paint` is false, so we never make our context
            // current at all — eliding that switch pair is the whole win.
            let now = elapsed_secs();
            if self.should_paint(now) {
                let t = prof.then(Instant::now);
                if unsafe { wglMakeCurrent(self.hdc, self.our_ctx) } == 0 {
                    log("wglMakeCurrent(dedicated context) failed in frame");
                    return;
                }
                if let Some(t) = t {
                    self.perf.rec(Sec::McTo, t.elapsed().as_nanos() as u64);
                }
                let resized = self.offscreen_size != (w, h);
                if resized {
                    let t = prof.then(Instant::now);
                    self.gr.reset(None);
                    if let Some(t) = t {
                        self.perf.rec(Sec::Reset, t.elapsed().as_nanos() as u64);
                    }
                }
                unsafe { gl::Viewport(0, 0, w, h) };
                self.ensure_offscreen(w, h);
                let pt = prof.then(Instant::now);
                let painted = self.paint(now, w, h);
                if let (Some(pt), true) = (pt, painted) {
                    self.perf.rec(Sec::Paint, pt.elapsed().as_nanos() as u64);
                    self.perf.note_paint();
                }
                // Submit Skia's writes to `hud_tex` before Minecraft's context
                // samples it. (Same-thread + flush ⇒ coherent; worst case the
                // HUD shows one frame late, which the 60 Hz cap makes invisible.)
                let ft = prof.then(Instant::now);
                self.gr.flush_and_submit();
                // DIAGNOSTIC: `ewo-comp-finish` sentinel → a hard GPU sync so the
                // texture render is guaranteed complete before Minecraft's
                // context samples it (tests the cross-context coherency theory).
                if std::env::temp_dir().join("ewo-comp-finish").exists() {
                    unsafe { gl::Finish() };
                }
                if let Some(ft) = ft {
                    self.perf.rec(Sec::Flush, ft.elapsed().as_nanos() as u64);
                }
                let t = prof.then(Instant::now);
                unsafe { wglMakeCurrent(self.hdc, self.mc_ctx) };
                if let Some(t) = t {
                    self.perf.rec(Sec::McBack, t.elapsed().as_nanos() as u64);
                }
            }
            // Composite the HUD texture onto Minecraft's framebuffer in its own
            // context — every frame, no switch.
            let bt = prof.then(Instant::now);
            self.composite_mc(w, h);
            if let Some(bt) = bt {
                self.perf.rec(Sec::Blit, bt.elapsed().as_nanos() as u64);
            }
        }

        if let Some(it) = injected_t {
            self.perf.rec(Sec::Injected, it.elapsed().as_nanos() as u64);
        }
    }

    /// Whether the paint clock is due (≥ `1 / paint_rate` since the last paint).
    /// Lets the fast path skip the context switch entirely on non-paint frames.
    fn should_paint(&self, now: f32) -> bool {
        now - self.last_painted >= self.editor.paint_rate().min_interval()
    }

    /// Composite `hud_tex` onto Minecraft's default framebuffer as a full-screen
    /// quad, drawn in **Minecraft's** GL context (no `wglMakeCurrent`). Every GL
    /// state bit we touch is saved and restored so Minecraft's own renderer —
    /// and its Java-side state cache — sees the context exactly as it left it.
    fn composite_mc(&mut self, w: i32, h: i32) {
        if self.hud_tex == 0 || self.comp_program == 0 {
            return; // nothing painted yet, or shader failed — show no HUD
        }
        // VAOs are not shared between contexts — create ours lazily here, in
        // Minecraft's context, the first time we composite. Same for the sampler
        // object that guarantees texture completeness.
        if self.comp_vao == 0 {
            unsafe { gl::GenVertexArrays(1, &mut self.comp_vao) };
            if self.comp_vao == 0 {
                return;
            }
        }
        if self.comp_sampler == 0 {
            unsafe {
                gl::GenSamplers(1, &mut self.comp_sampler);
                gl::SamplerParameteri(self.comp_sampler, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
                gl::SamplerParameteri(self.comp_sampler, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
                gl::SamplerParameteri(
                    self.comp_sampler,
                    gl::TEXTURE_WRAP_S,
                    gl::CLAMP_TO_EDGE as i32,
                );
                gl::SamplerParameteri(
                    self.comp_sampler,
                    gl::TEXTURE_WRAP_T,
                    gl::CLAMP_TO_EDGE as i32,
                );
            }
            if self.comp_sampler == 0 {
                return;
            }
        }

        unsafe {
            // ── save the state we are about to clobber ──
            let mut prev_prog = 0i32;
            gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut prev_prog);
            let mut prev_vao = 0i32;
            gl::GetIntegerv(gl::VERTEX_ARRAY_BINDING, &mut prev_vao);
            let mut prev_active = 0i32;
            gl::GetIntegerv(gl::ACTIVE_TEXTURE, &mut prev_active);
            let mut prev_fbo = 0i32;
            gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut prev_fbo);
            let mut prev_vp = [0i32; 4];
            gl::GetIntegerv(gl::VIEWPORT, prev_vp.as_mut_ptr());
            let blend_on = gl::IsEnabled(gl::BLEND);
            let depth_on = gl::IsEnabled(gl::DEPTH_TEST);
            let cull_on = gl::IsEnabled(gl::CULL_FACE);
            let scissor_on = gl::IsEnabled(gl::SCISSOR_TEST);
            let srgb_on = gl::IsEnabled(gl::FRAMEBUFFER_SRGB);
            let mut b_src_rgb = 0i32;
            let mut b_dst_rgb = 0i32;
            let mut b_src_a = 0i32;
            let mut b_dst_a = 0i32;
            gl::GetIntegerv(gl::BLEND_SRC_RGB, &mut b_src_rgb);
            gl::GetIntegerv(gl::BLEND_DST_RGB, &mut b_dst_rgb);
            gl::GetIntegerv(gl::BLEND_SRC_ALPHA, &mut b_src_a);
            gl::GetIntegerv(gl::BLEND_DST_ALPHA, &mut b_dst_a);
            let mut b_eq_rgb = 0i32;
            let mut b_eq_a = 0i32;
            gl::GetIntegerv(gl::BLEND_EQUATION_RGB, &mut b_eq_rgb);
            gl::GetIntegerv(gl::BLEND_EQUATION_ALPHA, &mut b_eq_a);
            let mut cmask = [0u8; 4];
            gl::GetBooleanv(gl::COLOR_WRITEMASK, cmask.as_mut_ptr());
            gl::ActiveTexture(gl::TEXTURE0);
            let mut prev_tex0 = 0i32;
            gl::GetIntegerv(gl::TEXTURE_BINDING_2D, &mut prev_tex0);
            let mut prev_sampler = 0i32;
            gl::GetIntegerv(gl::SAMPLER_BINDING, &mut prev_sampler);

            // ── set our state + draw ──
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
            gl::Viewport(0, 0, w, h);

            // Grab the world for the glass *before* the HUD quad goes down.
            // fbo 0 is bound and holds Minecraft's finished frame; one more
            // draw and it would hold ours too, which the glass would then
            // refract back into itself as a feedback smear at every rim.
            self.capture_game(w, h, elapsed_secs());
            gl::Disable(gl::DEPTH_TEST);
            gl::Disable(gl::CULL_FACE);
            gl::Disable(gl::SCISSOR_TEST);
            gl::Disable(gl::FRAMEBUFFER_SRGB); // Skia paints non-sRGB RGBA8 — write straight
            gl::Enable(gl::BLEND);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(
                gl::ONE,
                gl::ONE_MINUS_SRC_ALPHA,
                gl::ONE,
                gl::ONE_MINUS_SRC_ALPHA,
            );
            gl::ColorMask(gl::TRUE, gl::TRUE, gl::TRUE, gl::TRUE);
            gl::UseProgram(self.comp_program);
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, self.hud_tex);
            gl::BindSampler(0, self.comp_sampler); // guarantees texture completeness
            gl::Uniform1i(self.comp_tex_loc, 0);
            // DIAGNOSTIC: `ewo-comp-solid` sentinel → output a fixed
            // semi-transparent green instead of sampling, isolating the
            // blend/state path from the texture-sampling path.
            let solid = std::env::temp_dir().join("ewo-comp-solid").exists();
            gl::Uniform1i(self.comp_solid_loc, solid as i32);
            // Drain any pre-existing GL error so the post-draw check is ours.
            if self.composites < 4 {
                let _ = gl::GetError();
            }
            gl::BindVertexArray(self.comp_vao);
            gl::DrawArrays(gl::TRIANGLES, 0, 3);
            if self.composites < 4 {
                let err = gl::GetError();
                if err != gl::NO_ERROR {
                    log(&format!("composite_mc GL error after draw: 0x{err:x}"));
                }
            }

            // ── restore Minecraft's state exactly ──
            gl::BindVertexArray(prev_vao as u32);
            gl::BindSampler(0, prev_sampler as u32);
            gl::BindTexture(gl::TEXTURE_2D, prev_tex0 as u32);
            gl::ActiveTexture(prev_active as u32);
            gl::UseProgram(prev_prog as u32);
            gl::BindFramebuffer(gl::FRAMEBUFFER, prev_fbo as u32);
            gl::Viewport(prev_vp[0], prev_vp[1], prev_vp[2], prev_vp[3]);
            gl::BlendFuncSeparate(
                b_src_rgb as u32,
                b_dst_rgb as u32,
                b_src_a as u32,
                b_dst_a as u32,
            );
            gl::BlendEquationSeparate(b_eq_rgb as u32, b_eq_a as u32);
            if blend_on == 0 {
                gl::Disable(gl::BLEND);
            }
            if depth_on != 0 {
                gl::Enable(gl::DEPTH_TEST);
            }
            if cull_on != 0 {
                gl::Enable(gl::CULL_FACE);
            }
            if scissor_on != 0 {
                gl::Enable(gl::SCISSOR_TEST);
            }
            if srgb_on != 0 {
                gl::Enable(gl::FRAMEBUFFER_SRGB);
            }
            gl::ColorMask(cmask[0], cmask[1], cmask[2], cmask[3]);
        }

        self.composites += 1;
        if self.composites == 1 {
            log(&format!(
                "first MC-context composite ({w}x{h}) — HUD quad in Minecraft's context"
            ));
        }
    }

    /// Create (or recreate, on window resize) the offscreen GPU surface the HUD
    /// is painted to. No-op when one of the right size already exists.
    ///
    /// We back it with a GL texture **we** allocate (rather than a Skia-managed
    /// render target) so its id is stable and — thanks to `wglShareLists` — can
    /// be sampled from Minecraft's context by `composite_mc`. Called only while
    /// our context is current.
    fn ensure_offscreen(&mut self, w: i32, h: i32) {
        if self.offscreen.is_some() && self.offscreen_size == (w, h) {
            return;
        }
        // Drop the old surface first (releases Skia's FBO wrapper), then the
        // texture it wrapped.
        self.offscreen = None;
        if self.hud_tex != 0 {
            unsafe { gl::DeleteTextures(1, &self.hud_tex) };
            self.hud_tex = 0;
        }

        let mut tex = 0u32;
        unsafe {
            gl::GenTextures(1, &mut tex);
            gl::BindTexture(gl::TEXTURE_2D, tex);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                w,
                h,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                ptr::null(),
            );
            // 1:1 sample (window-sized texture, window-sized quad) — NEAREST is
            // exact; clamp so the edge texel never wraps.
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            // Single mip level — keeps the texture mipmap-complete for sampling
            // (the default MAX_LEVEL is 1000, which would demand mips we don't have).
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_BASE_LEVEL, 0);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAX_LEVEL, 0);
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }

        let info = TextureInfo {
            target: gl::TEXTURE_2D,
            id: tex,
            format: gl::RGBA8,
            protected: Protected::No,
        };
        // SAFETY: `tex` is a valid, just-allocated RGBA8 texture of size (w, h).
        let backend = unsafe { backend_textures::make_gl((w, h), Mipmapped::No, info, "ewo-hud") };
        self.offscreen = surfaces::wrap_backend_texture(
            &mut self.gr,
            &backend,
            SurfaceOrigin::TopLeft,
            0, // sample count
            ColorType::RGBA8888,
            None,
            None,
        );

        match &self.offscreen {
            Some(_) => {
                self.hud_tex = tex;
                self.offscreen_size = (w, h);
                log(&format!("offscreen HUD texture created ({w}x{h}, gl id {tex})"));
            }
            None => {
                unsafe { gl::DeleteTextures(1, &tex) };
                self.hud_tex = 0;
                self.offscreen_size = (0, 0);
                log("offscreen HUD texture wrap failed");
            }
        }
    }

    /// Create (or recreate, on resize) the glass capture texture, its Skia
    /// wrapper, and the two blur surfaces. Runs in *our* GL context.
    fn ensure_glass_surfaces(&mut self, w: i32, h: i32) {
        if self.glass_size == (w, h) && self.game_surface.is_some() {
            return;
        }
        // Release the old wrapper before its texture.
        self.game_surface = None;
        self.glass_rim = None;
        self.glass_frost = None;
        // A capture from the old size would be sampled at the new one.
        self.last_game = None;
        self.last_captured = f32::NEG_INFINITY;
        if self.game_tex != 0 {
            unsafe { gl::DeleteTextures(1, &self.game_tex) };
            self.game_tex = 0;
        }

        let mut tex = 0u32;
        unsafe {
            gl::GenTextures(1, &mut tex);
            gl::BindTexture(gl::TEXTURE_2D, tex);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                w,
                h,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                ptr::null(),
            );
            // LINEAR — this texture is minified into the blur surfaces, so a
            // filtered fetch is what we want (NEAREST would alias the downscale).
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_BASE_LEVEL, 0);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAX_LEVEL, 0);
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }

        let info = TextureInfo {
            target: gl::TEXTURE_2D,
            id: tex,
            format: gl::RGBA8,
            protected: Protected::No,
        };
        // SAFETY: `tex` is a valid, just-allocated RGBA8 texture of size (w, h).
        let backend = unsafe { backend_textures::make_gl((w, h), Mipmapped::No, info, "ewo-game") };
        // `BottomLeft` — see the field doc. A `TopLeft` wrap here would flip
        // every refraction vertically, which is exactly the kind of bug that
        // looks like "the shader is wrong".
        self.game_surface = surfaces::wrap_backend_texture(
            &mut self.gr,
            &backend,
            SurfaceOrigin::BottomLeft,
            0,
            ColorType::RGBA8888,
            None,
            None,
        );

        if self.game_surface.is_none() {
            unsafe { gl::DeleteTextures(1, &tex) };
            self.game_tex = 0;
            self.glass_size = (0, 0);
            log("glass: game texture wrap failed — falling back to flat plates");
            return;
        }
        self.game_tex = tex;
        self.glass_rim = gpu_surface(&mut self.gr, (w / 2).max(1), (h / 2).max(1));
        self.glass_frost = gpu_surface(&mut self.gr, (w / 4).max(1), (h / 4).max(1));
        self.glass_size = (w, h);
    }

    /// Copy the live framebuffer into [`Self::game_tex`].
    ///
    /// **Must be called with fbo 0 bound for reading and before the HUD is
    /// composited** — capturing after the blit would make the glass refract
    /// its own previous frame, a visible feedback smear at the rims.
    ///
    /// Safe to call from either GL context: the texture is shared via
    /// `wglShareLists`, and this touches only the 2D texture binding, which it
    /// restores.
    fn capture_game(&mut self, w: i32, h: i32, now: f32) {
        if self.game_tex == 0 || self.glass_size != (w, h) {
            return;
        }
        // Capture at the paint cadence — no point refreshing a backdrop that
        // will not be redrawn.
        if now - self.last_captured < self.editor.paint_rate().min_interval() {
            return;
        }
        unsafe {
            let mut prev_tex = 0i32;
            gl::GetIntegerv(gl::TEXTURE_BINDING_2D, &mut prev_tex);
            gl::BindTexture(gl::TEXTURE_2D, self.game_tex);
            gl::CopyTexSubImage2D(gl::TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
            gl::BindTexture(gl::TEXTURE_2D, prev_tex as u32);
        }
        self.last_captured = now;
    }

    /// Rebuild the two blur levels from the last capture. Runs in our context,
    /// at paint time. `None` disables glass for this frame — the plates then
    /// draw their flat chrome, which is why every failure here is a `return`
    /// and not a panic.
    fn glass_sources(&mut self, w: i32, h: i32) -> Option<hud::GlassSource> {
        if self.editor.glass_strength() <= 0.0 || self.glass_size != (w, h) {
            return None;
        }
        // Nothing captured yet — first frame after launch or a resize.
        if self.last_captured.is_infinite() {
            return None;
        }
        // Slow path parked a Skia snapshot; fast path left the shared texture
        // updated for us to snapshot here, in our own context.
        let game = match self.last_game.clone() {
            Some(image) => image,
            None => self.game_surface.as_mut()?.image_snapshot(),
        };

        // Rim: half resolution, light blur. Must retain structure — refraction
        // of featureless pixels is invisible.
        {
            let rim = self.glass_rim.as_mut()?;
            let (rw, rh) = ((w / 2).max(1) as f32, (h / 2).max(1) as f32);
            let mut p = Paint::default();
            p.set_image_filter(image_filters::blur((2.0, 2.0), TileMode::Clamp, None, None));
            let c = rim.canvas();
            c.clear(Color::TRANSPARENT);
            c.draw_image_rect(&game, None, Rect::from_wh(rw, rh), &p);
        }
        // Frost: quarter resolution, heavy blur. Text sits on this, so whatever
        // shows through has to be mush.
        {
            let frost = self.glass_frost.as_mut()?;
            let (fw, fh) = ((w / 4).max(1) as f32, (h / 4).max(1) as f32);
            let mut p = Paint::default();
            p.set_image_filter(image_filters::blur((4.0, 4.0), TileMode::Clamp, None, None));
            let c = frost.canvas();
            c.clear(Color::TRANSPARENT);
            c.draw_image_rect(&game, None, Rect::from_wh(fw, fh), &p);
        }

        Some(hud::GlassSource {
            rim: self.glass_rim.as_mut()?.image_snapshot(),
            rim_scale: 0.5,
            frost: self.glass_frost.as_mut()?.image_snapshot(),
            frost_scale: 0.25,
        })
    }

    /// Paint clock. Render the HUD onto the offscreen surface — but only if at
    /// least `1 / paint_rate` seconds have passed since the last paint. When
    /// gated out, the offscreen surface keeps its prior contents and
    /// `composite` simply re-blits the stale image.
    fn paint(&mut self, now: f32, w: i32, h: i32) -> bool {
        if now - self.last_painted < self.editor.paint_rate().min_interval() {
            return false; // capped — composite reuses the offscreen surface as-is
        }

        // Build the frame context *before* borrowing the offscreen canvas —
        // `glass_sources` needs `&mut self` (it renders into the blur surfaces)
        // and the canvas borrow would still be live.
        let frame = hud::Frame {
            time: now,
            // From the game snapshot the previous composite captured. `None` on
            // the first paint, after a resize, or if the capture path failed;
            // plates then fall back to their flat chrome.
            glass: self.glass_sources(w, h),
            glass_strength: self.editor.glass_strength(),
        };

        let Some(surface) = self.offscreen.as_mut() else {
            return false;
        };
        let canvas = surface.canvas();
        // Clear to transparent so only the widgets composite over the game.
        canvas.clear(Color::TRANSPARENT);

        if self.buffer != 0 {
            // SAFETY: `buffer` is the address of the mod's direct `ByteBuffer`,
            // held for the process lifetime (`EwoHudData.CAPACITY` bytes).
            let data = unsafe { hud::HudData::new(self.buffer as *const u8) };
            if data.schema_version() == hud::SCHEMA_VERSION {
                hud::draw(
                    canvas,
                    &data,
                    &mut self.editor,
                    &self.font_store,
                    w as f32,
                    h as f32,
                    frame,
                );
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
        true
    }

    /// Create (or recreate, on window resize) the two GPU surfaces the cached
    /// frost is built in: a half-resolution scratch surface and the
    /// quarter-resolution cache itself.
    fn ensure_frost_surfaces(&mut self, w: i32, h: i32) {
        if self.frost.is_some() && self.frost_size == (w, h) {
            return;
        }
        self.frost_half = gpu_surface(&mut self.gr, (w / 2).max(1), (h / 2).max(1));
        self.frost = gpu_surface(&mut self.gr, (w / 4).max(1), (h / 4).max(1));
        self.frost_size = if self.frost.is_some() && self.frost_half.is_some() {
            (w, h)
        } else {
            (0, 0)
        };
        self.last_frosted = f32::NEG_INFINITY;
    }

    /// Frost clock — the third, slowest clock. Recompute the cached frosted
    /// backdrop from `game` (a snapshot of the live framebuffer), but only
    /// when the cache has gone stale (`FROST_REFRESH_INTERVAL`). The blur is a
    /// clean two-step 2× downscale to quarter resolution (each linear 2× step
    /// averages an exact 2×2 block, so it never aliases) plus a small gaussian
    /// on that small surface. `composite` upscales the result with a cubic
    /// resampler — together that reads as a smooth wide gaussian without the
    /// block artifacts a single big full-res blur kernel produces.
    fn refresh_frost(&mut self, game: &Image, now: f32) {
        if now - self.last_frosted < FROST_REFRESH_INTERVAL {
            return; // cache still fresh — composite reuses it as-is
        }
        let (w, h) = self.frost_size;
        if w == 0 || h == 0 {
            return; // surfaces failed to allocate
        }

        // Step 1: clean 2× downscale  game → half-res scratch.
        let half_img = {
            let Some(half) = self.frost_half.as_mut() else {
                return;
            };
            let dst = Rect::from_wh((w / 2).max(1) as f32, (h / 2).max(1) as f32);
            let canvas = half.canvas();
            canvas.clear(Color::TRANSPARENT);
            canvas.draw_image_rect_with_sampling_options(
                game,
                None,
                dst,
                FilterMode::Linear,
                &Paint::default(),
            );
            half.image_snapshot()
        };

        // Step 2: clean 2× downscale + a small gaussian  half-res → quarter-res cache.
        {
            let Some(quarter) = self.frost.as_mut() else {
                return;
            };
            let dst = Rect::from_wh((w / 4).max(1) as f32, (h / 4).max(1) as f32);
            let mut blur = Paint::default();
            blur.set_image_filter(image_filters::blur((3.0, 3.0), TileMode::Clamp, None, None));
            let canvas = quarter.canvas();
            canvas.clear(Color::TRANSPARENT);
            canvas.draw_image_rect_with_sampling_options(
                &half_img,
                None,
                dst,
                FilterMode::Linear,
                &blur,
            );
        }
        self.last_frosted = now;
    }

    /// Composite clock. Blit the offscreen surface onto Minecraft's
    /// framebuffer (`fbo 0`). Runs every frame so the HUD is always shown,
    /// even on frames where `paint` was rate-gated. When `frost` is set the
    /// cached frosted backdrop is upscaled in first, so the overlay reads as
    /// glass over depth.
    fn composite(&mut self, w: i32, h: i32, frost: bool) {
        let prof = self.perf.enabled;
        let wt = prof.then(Instant::now);
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
        if let Some(wt) = wt {
            self.perf.rec(Sec::Wrap, wt.elapsed().as_nanos() as u64);
        }

        // Capture the world for the glass, before the frost and before the HUD
        // blit. Before the *frost* specifically: the glass wants structure at
        // its rim to bend, and the frost has already destroyed it. Before the
        // *blit* for the same reason as the fast path — otherwise the glass
        // refracts its own previous frame.
        {
            let now = elapsed_secs();
            if now - self.last_captured >= self.editor.paint_rate().min_interval()
                && self.glass_size == (w, h)
                && self.editor.glass_strength() > 0.0
            {
                self.last_game = Some(fbo.image_snapshot());
                self.last_captured = now;
            }
        }

        if frost {
            // Frost the live game before the HUD is blitted on top, so the
            // overlay's glass panels read against blurred depth. The blur is a
            // *cached* quarter-res surface (see `refresh_frost`) recomputed
            // only a few times a second; here it is just upscaled in — one
            // textured quad, cheap enough to run every composite.
            self.ensure_frost_surfaces(w, h);
            let game = fbo.image_snapshot();
            self.refresh_frost(&game, elapsed_secs());
            if let Some(quarter) = self.frost.as_mut() {
                let blurred = quarter.image_snapshot();
                let dst = Rect::from_wh(w as f32, h as f32);
                // Cubic (Mitchell) upscale — smooth, no block artifacts from
                // the 4× magnification of the quarter-res cache.
                fbo.canvas().draw_image_rect_with_sampling_options(
                    &blurred,
                    None,
                    dst,
                    CubicResampler::mitchell(),
                    &Paint::default(),
                );
                // A faint Velvet wine wash deepens the frost so the overlay
                // panels read as glass over depth — the prototype's modal shroud.
                let mut tint = Paint::default();
                tint.set_color(Color::from_argb(70, 10, 0, 6));
                fbo.canvas().draw_rect(dst, &tint);
            }
            // Live-game cutouts. The CROSSHAIR editor's preview panes want
            // *un-frosted* live game underneath so the crosshair sits over
            // the real world at 1:1 (true-to-life). Each cutout re-draws
            // the pre-frost snapshot back into a rounded-rect region; the
            // offscreen Skia surface leaves the pane interior transparent
            // so the game stays visible there once the offscreen is
            // composited on top.
            let cutouts = self.editor.live_game_cutouts(w as f32, h as f32);
            for rect in &cutouts {
                let saved = fbo.canvas().save();
                fbo.canvas().clip_rrect(
                    RRect::new_rect_xy(*rect, 10.0, 10.0),
                    Some(ClipOp::Intersect),
                    Some(true),
                );
                fbo.canvas().draw_image_rect_with_sampling_options(
                    &game,
                    Some((rect, SrcRectConstraint::Strict)),
                    *rect,
                    FilterMode::Nearest,
                    &Paint::default(),
                );
                fbo.canvas().restore_to_count(saved);
            }
        }

        let bt = prof.then(Instant::now);
        if let Some(offscreen) = self.offscreen.as_mut() {
            // `image_snapshot` is copy-on-write: this snapshot is dropped at the
            // end of the frame, so the next `paint` renders into the offscreen
            // surface in place (no texture copy).
            let image = offscreen.image_snapshot();
            fbo.canvas().draw_image(&image, (0.0, 0.0), None);
        }
        if let Some(bt) = bt {
            self.perf.rec(Sec::Blit, bt.elapsed().as_nanos() as u64);
        }

        let ft = prof.then(Instant::now);
        self.gr.flush_and_submit();
        if let Some(ft) = ft {
            self.perf.rec(Sec::Flush, ft.elapsed().as_nanos() as u64);
        }

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

/// Hard process termination for the mod's exit watchdog.
///
/// The JVM's shutdown can deadlock in native teardown (DLL_PROCESS_DETACH
/// under the Windows loader lock — the 2nd GL context + the WinRT SMTC
/// media thread are the suspects), leaving a windowless zombie java.exe
/// that holds this dll + the instance files. `TerminateProcess` skips DLL
/// detach entirely, so it works even mid-deadlock. Only called by the
/// watchdog seconds after orderly shutdown (world saves included) finished.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeForceExit(
    _env: *mut c_void,
    _class: *mut c_void,
) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    #[cfg(not(windows))]
    std::process::exit(0);
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
                hud.frame(
                    HUD_BUFFER.load(Ordering::Relaxed),
                    MODULE_BUFFER.load(Ordering::Relaxed),
                );
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
    button: i32,
    pressed: u8,
    x: f64,
    y: f64,
) {
    with_hud(|hud| hud.editor.on_mouse_button(button, pressed != 0, x as f32, y as f32));
}

/// Overlay input — the scroll wheel. Drives the MODULES-tab vertical scroll
/// (the only dashboard view tall enough to overflow). Mouse wheel delta y is
/// positive when scrolling up — invert so positive scroll moves content up
/// (showing later rows).
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeMouseScroll(
    _env: *mut c_void,
    _class: *mut c_void,
    _dx: f64,
    dy: f64,
) {
    with_hud(|hud| hud.editor.on_scroll(dy as f32));
}

/// Mouse click forwarded by the Fabric mod when a *vanilla* screen is open
/// and the user left-clicks. Hit-tests the in-world Media widget's transport
/// buttons; if a button is hit the action fires and we return `1` so the mod
/// can cancel the vanilla screen's click. `0` means "not consumed — let
/// vanilla have it."
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeMediaTryClick(
    _env: *mut c_void,
    _class: *mut c_void,
    button: i32,
    x: f64,
    y: f64,
) -> u8 {
    let mut consumed: u8 = 0;
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        HUD.with(|cell| {
            if let HudState::Ready(hud) = &mut *cell.borrow_mut() {
                if hud.editor.try_media_click(button, x as f32, y as f32) {
                    consumed = 1;
                }
            }
        });
    }));
    consumed
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

/// Returns `1` when the custom crosshair is enabled and the Java mixin
/// should cancel vanilla's `Gui.extractCrosshair` so only ours draws,
/// `0` otherwise. Called once per frame from the suppression mixin.
///
/// Defaults to `0` whenever the HUD isn't initialised (e.g. before
/// `nativeInit`) so the vanilla crosshair always shows when no custom
/// override is in play.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeIsCustomCrosshairEnabled(
    _env: *mut c_void,
    _class: *mut c_void,
) -> u8 {
    let mut enabled: u8 = 0;
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        HUD.with(|cell| {
            if let HudState::Ready(hud) = &*cell.borrow() {
                if hud.editor.crosshair_config().enabled {
                    enabled = 1;
                }
            }
        });
    }));
    enabled
}

/// Quick-edit gate. Called every frame from `EwoQuickEdit`: `1` while the
/// modifier is held over a cursor-free vanilla screen, `0` otherwise.
///
/// Rust owns the mode because it owns the layout — Java only reports whether
/// the conditions are met, and the transition out of the mode is where an
/// in-progress drag gets committed.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeQuickEdit(
    _env: *mut c_void,
    _class: *mut c_void,
    on: u8,
) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        with_hud(|hud| hud.editor.set_quick_edit(on != 0));
    }));
}

/// Register the Rust→JVM module-state block (Phase G). Called once at mod init
/// with a direct `ByteBuffer`; Rust resolves its address and writes the block
/// every frame thereafter.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeInitModules(
    env: *mut jni_sys::JNIEnv,
    _class: *mut c_void,
    buf: jni_sys::jobject,
) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if env.is_null() || buf.is_null() {
            log("nativeInitModules: null env or buffer");
            return;
        }
        // SAFETY: documented JNI contract — identical to `nativeInit`.
        let addr = unsafe {
            match (**env).GetDirectBufferAddress {
                Some(get_addr) => get_addr(env, buf),
                None => {
                    log("nativeInitModules: GetDirectBufferAddress unavailable");
                    return;
                }
            }
        };
        if addr.is_null() {
            log("nativeInitModules: GetDirectBufferAddress returned null (buffer not direct?)");
            return;
        }
        MODULE_BUFFER.store(addr as usize, Ordering::Relaxed);
        log("nativeInitModules: module-state block registered");
    }));
}

/// Flip module `index`'s enabled flag (Phase G). Called from the Java key
/// handler when a module's toggle key is pressed — Rust owns module state, so
/// the keypress round-trips through here; the next frame's buffer write carries
/// the new state back to the mod.
#[no_mangle]
pub extern "system" fn Java_dev_lewlone_ewohud_EwoHudNative_nativeModuleToggle(
    _env: *mut c_void,
    _class: *mut c_void,
    index: i32,
) {
    if index < 0 {
        return;
    }
    with_hud(|hud| hud.editor.modules.toggle(index as usize));
}
