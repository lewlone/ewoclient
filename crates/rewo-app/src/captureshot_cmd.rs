//! `rewo captureshot --check` — the screenshot-capture oracle (M51c).
//!
//! M51b shipped F2 and recorded the hole in as many words: *no gate exercises a
//! BGRA `Offscreen`*. All sixteen existing `Offscreen` call sites take the
//! `R8G8B8A8_SRGB` default, so the whole suite is structurally blind to the one
//! branch that decides whether every screenshot this client will ever take has
//! its red and blue exchanged.
//!
//! # Why the branch exists, and why it needs its own gate
//!
//! `Swapchain::new` prefers `B8G8R8A8_SRGB`, and `WorldRenderer` bakes its
//! colour format into every pipeline it builds — so a capture taken from the
//! live client must render into a **BGRA** attachment or not render at all.
//! `read_rgba` then copies that image's memory verbatim, which is B,G,R,A,
//! while `png::ColorType::Rgba` means R,G,B,A. `save_png` swizzles when
//! `is_bgra()`.
//!
//! Nothing about that is self-evidently right. Two failure modes are equally
//! plausible and both produce a plausible-looking picture:
//!
//! * the swizzle is **missing** (M51a's own worry) — every live screenshot has
//!   red and blue swapped;
//! * the swizzle is **spurious** — if the copy did not actually hand back BGRA
//!   bytes, applying it would *introduce* the swap it claims to fix.
//!
//! [`check_swizzle`] settles it by measuring both halves: `a2` observes the raw
//! readback and proves the bytes really do arrive permuted (the fault), and
//! `a1`/`a3` prove the saved file is nevertheless red-first and identical to the
//! RGBA path (the correction). Either witness alone would be worth little.
//!
//! # What else is in reach
//!
//! [`check_grab`] drives production `capture::grab` end to end at the live
//! client's format — the M45/M47 lesson, that a gate reimplementing a slice of
//! the app's setup misses whatever the app adds to it. It writes into the real
//! screenshots directory, because that is what F2 does, and removes what it
//! wrote.
//!
//! [`check_naming`] grades the transcribed rules — `Util`'s filename pattern and
//! `Screenshot.getFile`'s dedup ladder — through the real functions.
//!
//! **Out of reach, deliberately:** proving a capture matches what the *window*
//! presented. Rewo's swapchain images carry no `TRANSFER_SRC`, so there is
//! nothing to read back from a presented frame, and a gate cannot open a window
//! anyway. What is proved instead is that the capture path renders the same
//! frame a hand-driven `Offscreen` does at the same format (`b2`), which is the
//! composition `grab` documents.
//!
//! Needs no client jar and no server: the world texture array accepts an empty
//! layer slice (`upload_texture_array` substitutes one white layer), and every
//! colour this gate grades is a clear value or a sky uniform it chose itself.
//! An oracle whose expectation depends on an asset's contents is testing the
//! asset.

use std::path::{Path, PathBuf};

use ash::vk;
use clap::Args as ClapArgs;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;

use crate::capture;
use crate::stats::OverlayRing;

/// A witness that stops running is a failure, not a quieter pass.
const EXPECTED_WITNESSES: usize = 17;

const W: u32 = 128;
const H: u32 = 128;

/// The format a live capture actually uses — `Swapchain::new` asks for it by
/// name and only falls back if the surface refuses.
const LIVE: vk::Format = vk::Format::B8G8R8A8_SRGB;
/// The format every other gate uses, and `Offscreen::new`'s default.
const GATES: vk::Format = vk::Format::R8G8B8A8_SRGB;

/// A clear whose three colour channels are **strictly ordered** with wide gaps.
///
/// Only the ordering is load-bearing. Whatever an implementation does to a
/// clear value on an sRGB attachment it is monotonic, so `R > G > B` survives
/// it — and a byte-transposed write reverses it. `VkClearColorValue`'s four
/// floats map to the R, G, B and A components *of the format*, which is a
/// statement about components and not about memory layout: index 0 is red in a
/// BGRA image exactly as it is in an RGBA one.
const CLEAR_ORDERED: [f32; 4] = [1.0, 0.25, 0.0, 1.0];

/// The same colour at half alpha — the sensitivity partner for `a5`.
const CLEAR_TRANSLUCENT: [f32; 4] = [1.0, 0.25, 0.0, 0.5];

/// The overlay chart's rect, in framebuffer pixels: the top-left quadrant.
/// `gl_FragCoord`'s origin is the **upper left** in Vulkan and so is a
/// framebuffer's, so this is the anchor `a7` reads the row order against.
const CHART_ORIGIN: [f32; 2] = [8.0, 8.0];
const CHART_SIZE: [f32; 2] = [40.0, 40.0];

/// A pixel well clear of the chart, where the frame is exactly the clear value.
const CLEAR_PROBE: (u32, u32) = (100, 100);

/// The clear `capture::grab` uses, transcribed here rather than exported from
/// there — `b2` compares two renders, so a divergence in this constant shows up
/// as a difference rather than as two implementations agreeing with each other.
const GRAB_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A red-dominant sky base (LINEAR), so the frame `grab` captures is both
/// spatially varying and strongly ordered in R over B.
const SKY_BASE: [f32; 3] = [0.75, 0.16, 0.04];

#[derive(ClapArgs, Debug)]
pub struct CaptureshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    /// The sole, explicit opt-out from Vulkan validation.
    #[arg(long, default_value_t = false)]
    pub no_validation: bool,
    /// Keep the rendered PNGs here instead of a scratch directory, for
    /// eyeballing a failure.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[captureshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// -- rendering rig ------------------------------------------------------------

/// The overlay chart, parked in the top-left quadrant. `fill_demo` is
/// deterministic, so every frame this gate renders is reproducible.
fn overlay_chart(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: CHART_ORIGIN,
        size: CHART_SIZE,
    }
}

/// An overlay parked far offscreen, for the frames that must be sky alone.
fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

/// One capture: the raw readback, and the bytes that came back out of the file
/// `save_png` wrote.
struct Shot {
    raw: Vec<u8>,
    png: Vec<u8>,
    color_type: png::ColorType,
    dims: (u32, u32),
    path: PathBuf,
}

/// Render the clear-plus-chart frame at `format` and both read it back raw and
/// round-trip it through `save_png`.
fn shoot(
    gpu: &mut Gpu,
    format: vk::Format,
    clear: [f32; 4],
    draw: &OverlayDraw,
    dir: &Path,
    name: &str,
) -> Result<Shot, String> {
    let mut off = Offscreen::with_format(gpu, W, H, format)?;
    let res = (|| {
        off.render(gpu, None, draw, clear)?;
        let raw = off.read_rgba(gpu)?;
        let path = dir.join(format!("{name}.png"));
        off.save_png(gpu, &path)?;
        let (dims, color_type, png) = decode_png(&path)?;
        Ok(Shot {
            raw,
            png,
            color_type,
            dims,
            path,
        })
    })();
    // Destroy on the error path too — a leaked `Offscreen` is a fistful of
    // `VUID-vkDestroyDevice-device-05137`s, and this gate runs with validation
    // ON.
    off.destroy(gpu);
    res
}

/// A `WorldRenderer` painting nothing but a red-dominant gradient sky.
///
/// Built at the caller's format on purpose: `b1`'s whole point is that a
/// renderer and its attachment must agree, so this is the half `grab` has to
/// match.
fn sky_renderer(gpu: &mut Gpu, format: vk::Format) -> Result<WorldRenderer, String> {
    // An empty layer slice is honoured — `upload_texture_array` substitutes one
    // white layer — so no client jar is needed for a frame with no terrain in it.
    let mut wr = WorldRenderer::new(gpu, format, rewo_data::assets::TEX_SIZE, &[])?;
    wr.set_camera([0.0, 0.0, 0.0]);
    wr.set_sky_tint([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]);
    wr.set_sky_fog_base(SKY_BASE, SKY_BASE);
    Ok(wr)
}

/// A view-projection at the origin looking down −Z, level with the horizon.
fn view_proj() -> [[f32; 4]; 4] {
    let proj = perspective_reverse_z(70f32.to_radians(), W as f32 / H as f32, 0.05);
    let view = glam::Mat4::look_at_rh(
        glam::Vec3::ZERO,
        glam::Vec3::new(0.0, 0.0, -1.0),
        glam::Vec3::Y,
    );
    (glam::Mat4::from_cols_array_2d(&proj) * view).to_cols_array_2d()
}

// -- reading the artefact -----------------------------------------------------

/// Decode a PNG back to `((w, h), colour type, RGBA8 bytes)`.
///
/// No `Transformations` — the stored colour type is itself part of what
/// `save_png` is being graded on, and normalising it away would hide a change.
fn decode_png(path: &Path) -> Result<((u32, u32), png::ColorType, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("png info {path:?}: {e}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("png {path:?}: no output buffer size"))?;
    let mut buf = vec![0u8; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame {path:?}: {e}"))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("png {path:?}: bit depth {:?}", info.bit_depth));
    }
    buf.truncate(info.buffer_size());
    Ok(((info.width, info.height), info.color_type, buf))
}

fn px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// `a` with channels 0 and 2 exchanged in every texel — the permutation a BGRA
/// readback differs from an RGBA one by.
fn swap_rb(a: &[u8]) -> Vec<u8> {
    let mut v = a.to_vec();
    for t in v.chunks_exact_mut(4) {
        t.swap(0, 2);
    }
    v
}

fn bytes_differing(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

fn largest_byte_delta(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

// -- entry point --------------------------------------------------------------

pub fn run(args: CaptureshotArgs) -> Result<(), String> {
    // Validation is on in every build, debug AND release — `debug_assertions`
    // would silently drop it from the binary the gate actually ships as.
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[captureshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "captureshot check: Vulkan validation requested but not active — install \
             the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass --no-validation"
                .into(),
        );
    }
    run_check(&mut gpu, &args)
}

fn run_check(gpu: &mut Gpu, args: &CaptureshotArgs) -> Result<(), String> {
    let scratch = match &args.out_dir {
        Some(d) => d.clone(),
        None => std::env::temp_dir().join(format!("rewo-captureshot-{}", std::process::id())),
    };
    std::fs::create_dir_all(&scratch).map_err(|e| format!("scratch dir: {e}"))?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    let mut ring = OverlayRing::default();
    ring.fill_demo(0.0);

    let rendered = check_swizzle(&mut c, gpu, &ring, &scratch)
        .and_then(|()| check_grab(&mut c, gpu, &ring, &scratch));
    // The naming rules need no GPU and no successful render, so they run either
    // way — a failure above must not silently shrink the witness count into a
    // second, quieter kind of failure.
    check_naming(&mut c, &scratch)?;

    if args.out_dir.is_none() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    rendered?;

    println!(
        "[captureshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "captureshot observed {} witnesses, expected {EXPECTED_WITNESSES} — a \
             witness that stops running is a failure, not a quieter pass",
            c.witnessed
        ));
    }
    println!("[captureshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// -- Part A: the channel order ------------------------------------------------

/// The witnesses M51b said the gate owed.
fn check_swizzle(
    c: &mut Checker,
    gpu: &mut Gpu,
    ring: &OverlayRing,
    dir: &Path,
) -> Result<(), String> {
    let draw = overlay_chart(ring);
    let live = shoot(gpu, LIVE, CLEAR_ORDERED, &draw, dir, "bgra")?;
    let gates = shoot(gpu, GATES, CLEAR_ORDERED, &draw, dir, "rgba")?;

    // -- a1: the file is red-first ------------------------------------------
    let (x, y) = CLEAR_PROBE;
    let p = px(&live.png, x, y);
    let ordered = p[0] > p[1].saturating_add(20) && p[1] > p[2].saturating_add(20);
    c.record(
        "a1.a_bgra_capture_writes_red_first",
        ordered && live.color_type == png::ColorType::Rgba && live.dims == (W, H),
        format!(
            "a `B8G8R8A8_SRGB` capture cleared to a strictly R>G>B colour reads back \
             {p:?} at ({x},{y}) — red first, blue last, in a {:?} {:?} file. \
             `VkClearColorValue`'s floats map to the format's R,G,B,A *components*, \
             so index 0 is red whatever the memory layout; `png::ColorType::Rgba` \
             puts red at byte 0; and the clear's transfer function, whatever it is, \
             is monotonic, so the ordering survives it. MUTATION: delete the \
             `if self.is_bgra()` swap in `save_png` and this pixel comes back \
             blue-first — which is exactly what a live screenshot would have shipped \
             as, and would have looked perfectly plausible",
            live.dims, live.color_type
        ),
    );

    // -- a2: the fault the swizzle exists to correct --------------------------
    // Without this, a1 could be passing on a copy that already handed back RGBA,
    // in which case the swizzle would be an active bug rather than a fix. This
    // is the witness that decides which of those two worlds we are in.
    let swapped = swap_rb(&gates.raw);
    let raw_permuted = live.raw == swapped;
    let raw_identical = live.raw == gates.raw;
    c.record(
        "a2.the_bgra_readback_really_is_byte_swapped",
        raw_permuted && !raw_identical,
        format!(
            "the raw `read_rgba` of the BGRA target is the RGBA target's bytes with \
             channels 0 and 2 exchanged, texel for texel ({} of {} bytes differ \
             before the exchange, 0 after) — `cmd_copy_image_to_buffer` copies the \
             image's memory verbatim, and a `B8G8R8A8` image stores B,G,R,A. This is \
             the FAULT; a1 is the correction. Had these two come back identical, the \
             swizzle would be introducing the swap it claims to fix, and this witness \
             is the only thing in the suite that could have told the difference",
            bytes_differing(&live.raw, &gates.raw),
            gates.raw.len()
        ),
    );

    // -- a3: a capture must not depend on the swapchain's byte order ----------
    let same_file = live.png == gates.png;
    c.record(
        "a3.the_two_formats_produce_the_same_file",
        same_file,
        format!(
            "the PNG saved from the live client's `B8G8R8A8_SRGB` target is \
             byte-identical to the one saved from the gates' `R8G8B8A8_SRGB` target \
             ({} of {} bytes differ, largest delta {}) — the fifteen other Vulkan \
             gates pin the RGBA path against predicted values, so equality with it \
             carries all of that over to the format nobody else exercises. MUTATION: \
             make the swizzle unconditional and the RGBA path becomes the wrong one \
             instead, which this sees from the other side",
            bytes_differing(&live.png, &gates.png),
            gates.png.len(),
            largest_byte_delta(&live.png, &gates.png)
        ),
    );

    // -- a4: the classification table ----------------------------------------
    // `is_bgra` is a match on two format names. Getting it wrong in the
    // generous direction double-applies the swap to an RGBA capture; in the
    // stingy direction it misses a UNORM swapchain.
    let mut flags = Vec::new();
    for f in [
        vk::Format::B8G8R8A8_SRGB,
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_SRGB,
        vk::Format::R8G8B8A8_UNORM,
    ] {
        let mut off = Offscreen::with_format(gpu, 4, 4, f)?;
        flags.push((f, off.is_bgra()));
        off.destroy(gpu);
    }
    let want = [true, true, false, false];
    c.record(
        "a4.is_bgra_names_both_bgra_formats_and_neither_rgba_one",
        flags.iter().map(|(_, b)| *b).eq(want),
        format!(
            "on real targets: {}. Both `B8G8R8A8` variants swizzle and neither \
             `R8G8B8A8` one does. MUTATION: adding an RGBA format to the match arm \
             double-applies the swap (a3 fails); dropping the UNORM arm loses a \
             swapchain that negotiated `B8G8R8A8_UNORM`, which is the fallback \
             `Swapchain::new` takes when the surface refuses its first choice",
            flags
                .iter()
                .map(|(f, b)| format!("{f:?}={b}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );

    // -- a5/a6: alpha ---------------------------------------------------------
    // Vanilla forces `argb | 0xFF000000` in `takeScreenshot`, so a vanilla
    // screenshot is opaque by construction. Rewo does not, and this is the
    // witness that says it does not have to.
    let opaque = live.png.chunks_exact(4).all(|t| t[3] == 255);
    c.record(
        "a5.a_capture_is_opaque",
        opaque,
        format!(
            "every one of the {} texels in the saved file has alpha 255. \
             `Screenshot.takeScreenshot` ORs `0xFF000000` into every pixel; Rewo has \
             no such line, and does not need one because the world pass clears \
             opaque and the overlay pipeline masks alpha writes \
             (`color_write_mask` is R|G|B). MUTATION: drop that mask and the chart's \
             0.78 blend alpha ghosts straight through into the file",
            live.png.len() / 4
        ),
    );

    let translucent = shoot(gpu, LIVE, CLEAR_TRANSLUCENT, &draw, dir, "bgra-alpha")?;
    let ta = px(&translucent.png, CLEAR_PROBE.0, CLEAR_PROBE.1)[3];
    c.record(
        "a6.the_alpha_channel_is_genuinely_observed",
        ta > 100 && ta < 160,
        format!(
            "the same render at clear alpha 0.5 stores alpha {ta}, not 255 — so a5 is \
             a measured result and not a channel the encoder was hard-writing opaque. \
             Note the value is NOT sRGB-encoded (0.5 → ~128): alpha carries no \
             transfer function, which is why the colour channels and this one land at \
             different numbers from the same 0.5"
        ),
    );

    // -- a7: the row order ----------------------------------------------------
    // The claim `capture.rs` currently makes only in a comment.
    let clear_px = px(&live.png, CLEAR_PROBE.0, CLEAR_PROBE.1);
    let mut wrong = 0usize;
    let mut charted = 0usize;
    for y in 0..H {
        for x in 0..W {
            let p = px(&live.png, x, y);
            let differs = (0..3).any(|i| p[i].abs_diff(clear_px[i]) > 6);
            let inside = (8..48).contains(&x) && (8..48).contains(&y);
            if differs {
                charted += 1;
            }
            if differs != inside {
                wrong += 1;
            }
        }
    }
    c.record(
        "a7.the_capture_is_not_vertically_flipped",
        wrong == 0 && charted == 1600,
        format!(
            "the overlay chart at framebuffer origin (8,8) size 40x40 occupies \
             exactly the file's rows 8..47 and columns 8..47 — {charted} non-clear \
             texels, {wrong} in the wrong place. Vulkan puts a framebuffer's and \
             `gl_FragCoord`'s origin at the UPPER LEFT, `read_rgba` copies rows \
             tightly packed from image row 0, and the encoder writes row 0 first, so \
             the top of the frame must be the top of the file. \
             `Screenshot.takeScreenshot` writes `setPixelABGR(x, height - y - 1, …)` \
             because `glReadPixels` hands back a bottom-up image; MUTATION: copy that \
             inversion here — which is what it looks like you are missing when you \
             diff the two implementations — and the chart lands in rows 80..119"
        ),
    );

    let _ = (live.path, gates.path, translucent.path);
    Ok(())
}

// -- Part B: the production path ----------------------------------------------

/// Drive `capture::grab` itself, at the live client's format.
fn check_grab(c: &mut Checker, gpu: &mut Gpu, ring: &OverlayRing, dir: &Path) -> Result<(), String> {
    let draw = overlay_offscreen(ring);
    let vp = view_proj();

    // -- b1/b3: the real thing ------------------------------------------------
    let mut wr = sky_renderer(gpu, LIVE)?;
    let grabbed = capture::grab(gpu, &mut wr, vp, &draw, LIVE, vk::Extent2D {
        width: W,
        height: H,
    });
    wr.destroy(gpu);
    let path = grabbed?;
    let decoded = decode_png(&path);
    let named = capture::screenshot_dir().is_some_and(|d| path.parent() == Some(d.as_path()));
    let stem_ok = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(vanilla_shaped);

    let ((gw, gh), gct, grab_png) = decoded?;
    // Written by the gate, so removed by the gate — this is the user's real
    // screenshots directory, not a scratch one.
    let _ = std::fs::remove_file(&path);

    let red_first = {
        let p = [
            grab_png[(((H / 2) * W + W / 2) * 4) as usize],
            grab_png[(((H / 2) * W + W / 2) * 4 + 1) as usize],
            grab_png[(((H / 2) * W + W / 2) * 4 + 2) as usize],
        ];
        p[0] > p[1].saturating_add(20) && p[1] > p[2].saturating_add(20)
    };
    c.record(
        "b1.grab_builds_its_target_at_the_callers_format",
        (gw, gh) == (W, H) && gct == png::ColorType::Rgba && red_first,
        format!(
            "a `WorldRenderer` built for `B8G8R8A8_SRGB` captured through \
             `capture::grab` at that same format produced a {gw}x{gh} {gct:?} file \
             whose red-dominant sky is still red-dominant. `WorldRenderer::new` bakes \
             its colour format into every pipeline, so this only renders at all \
             because `grab` passes the caller's format to `Offscreen::with_format`. \
             MUTATION: call `Offscreen::new` instead and the attachment is RGBA while \
             the pipelines are BGRA — with validation ON that is a \
             `vkCmdBeginRendering` colour-format mismatch the layers refuse, not a \
             subtly wrong picture. (Spelled without the validation-id token on \
             purpose: this gate's own output is grepped for one)"
        ),
    );

    c.record(
        "b3.grab_names_the_file_the_way_vanilla_does",
        named && stem_ok,
        format!(
            "the capture landed at {path:?} — under `screenshot_dir()`, named \
             `yyyy-MM-dd_HH.mm.ss[_N].png`. Vanilla's `Screenshot.grab` makes \
             `new File(workDir, \"screenshots\")` and takes its name from \
             `Util.getFilenameFormattedDateTime()`; Rewo resolves the directory \
             through `dirs::config_dir()/EwoClient` because it has no game-directory \
             notion, which is the analogue rather than the literal path. The gate \
             wrote this into the user's real screenshots directory, because that is \
             what F2 does, and removed it again"
        ),
    );

    // -- b2: grab is the composition it documents -----------------------------
    let mut wr = sky_renderer(gpu, LIVE)?;
    let mut off = Offscreen::with_format(gpu, W, H, LIVE)?;
    let manual = (|| {
        off.render(gpu, Some((&mut wr, vp)), &draw, GRAB_CLEAR)?;
        let p = dir.join("grab-reference.png");
        off.save_png(gpu, &p)?;
        decode_png(&p)
    })();
    off.destroy(gpu);
    wr.destroy(gpu);
    let (_, _, manual_png) = manual?;

    // Guard: two black frames agreeing would prove nothing. A gradient sky has
    // a real vertical spread, which also shows the world pass ran at all.
    let top = px(&manual_png, W / 2, 4);
    let bottom = px(&manual_png, W / 2, H - 5);
    let varied = (0..3).map(|i| top[i].abs_diff(bottom[i])).max().unwrap_or(0);
    c.record(
        "b2.grabs_frame_is_the_one_a_hand_driven_offscreen_renders",
        grab_png == manual_png && varied > 8,
        format!(
            "`grab`'s file is byte-identical to a hand-driven \
             `with_format` + `render(Some(world))` + `save_png` of the same renderer, \
             view-projection and overlay ({} of {} bytes differ), and the frame is \
             not two black images agreeing — the gradient sky spans {varied} units \
             between row 4 and row {}. The clear is transcribed here \
             ({GRAB_CLEAR:?}) rather than exported from `capture.rs`, so a change to \
             `grab`'s own clear shows up as a difference instead of as two \
             implementations agreeing with each other",
            bytes_differing(&grab_png, &manual_png),
            manual_png.len(),
            H - 5
        ),
    );
    Ok(())
}

/// `yyyy-MM-dd_HH.mm.ss` optionally followed by `_N`, then `.png`.
fn vanilla_shaped(name: &str) -> bool {
    let Some(base) = name.strip_suffix(".png") else {
        return false;
    };
    let (stem, suffix) = match base.split_once('_') {
        Some((d, rest)) => match rest.split_once('_') {
            Some((t, n)) => (format!("{d}_{t}"), Some(n.to_string())),
            None => (format!("{d}_{rest}"), None),
        },
        None => return false,
    };
    if let Some(n) = suffix {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let mut parts = stem.split('_');
    let (Some(date), Some(time), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split('.').collect();
    d.len() == 3
        && t.len() == 3
        && d[0].len() == 4
        && d[1..].iter().chain(t.iter()).all(|f| f.len() == 2)
        && d.iter()
            .chain(t.iter())
            .all(|f| f.bytes().all(|b| b.is_ascii_digit()))
}

// -- Part C: the naming rules -------------------------------------------------

/// Characters Windows refuses in a filename. `:` is the one that matters here —
/// it is what a symmetric transcription of the time would have used.
const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

fn check_naming(c: &mut Checker, dir: &Path) -> Result<(), String> {
    // -- c1: the pattern ------------------------------------------------------
    let stem = capture::filename_stem(2026, 7, 28, 9, 4, 5);
    c.record(
        "c1.the_stem_separates_the_date_with_hyphens_and_the_time_with_dots",
        stem == "2026-07-28_09.04.05",
        format!(
            "`filename_stem(2026, 7, 28, 9, 4, 5)` is {stem:?}, zero-padded \
             throughout. `Util.FILENAME_DATE_TIME_FORMATTER` is \
             `DateTimeFormatter.ofPattern(\"yyyy-MM-dd_HH.mm.ss\", Locale.ROOT)` — the \
             separators are asymmetric, and that is not decorative. MUTATION: the \
             symmetric `HH:mm:ss` a transcription naturally reaches for (see c6)"
        ),
    );

    c.record(
        "c6.the_stem_carries_no_character_windows_refuses",
        !stem.contains(ILLEGAL),
        format!(
            "{stem:?} contains none of {ILLEGAL:?}. This is WHY the time uses dots: \
             `:` is illegal in a Windows filename, and on NTFS `name:x` is not even \
             an error — it opens an alternate data stream, so a symmetric \
             transcription would write a zero-byte file and a screenshot that \
             silently vanished. The rule is a portability constraint baked into \
             vanilla's format string, not a style choice"
        ),
    );

    // -- c2: the ladder -------------------------------------------------------
    let ladder_dir = dir.join("ladder");
    let _ = std::fs::remove_dir_all(&ladder_dir);
    std::fs::create_dir_all(&ladder_dir).map_err(|e| format!("ladder dir: {e}"))?;
    let s = "2026-07-28_09.04.05";
    let mut names = Vec::new();
    for _ in 0..3 {
        let p = capture::next_free_path(&ladder_dir, s);
        names.push(p.file_name().unwrap().to_string_lossy().into_owned());
        std::fs::write(&p, b"x").map_err(|e| format!("ladder write: {e}"))?;
    }
    c.record(
        "c2.the_dedup_ladder_leaves_the_first_bare_and_starts_at_two",
        names == ["2026-07-28_09.04.05.png", "2026-07-28_09.04.05_2.png", "2026-07-28_09.04.05_3.png"],
        format!(
            "three captures in the same second give {names:?}. \
             `Screenshot.getFile` is `name + (count == 1 ? \"\" : \"_\" + count) + \
             \".png\"` with `count` starting at 1 — it counts from one but OMITS the \
             suffix for one, so only a collision ever produces `_2`. MUTATION: start \
             the ladder at `_1` and every ordinary screenshot the client has ever \
             taken is renamed"
        ),
    );

    // -- c3/c4/c5: the civil conversion ---------------------------------------
    let epoch = capture::civil_from_unix(0, 0);
    let before = capture::civil_from_unix(-1, 0);
    c.record(
        "c3.the_civil_conversion_floors_before_the_epoch",
        epoch == (1970, 1, 1, 0, 0, 0) && before == (1969, 12, 31, 23, 59, 59),
        format!(
            "0 is {epoch:?} and −1 is {before:?} — one second before the epoch is the \
             last second of 1969, not a negative second of 1970. MUTATION: plain `/` \
             and `%` in place of `div_euclid`/`rem_euclid` truncate toward zero, so \
             the second-of-day comes out −1 and the `as u32` cast turns the stem into \
             `…4294967295`. A clock that is only ever read forward from now would \
             never have shown it"
        ),
    );

    let leap = capture::civil_from_unix(1_709_164_800, 0);
    c.record(
        "c5.the_leap_day_lands_on_february_29",
        leap == (2024, 2, 29, 0, 0, 0),
        format!(
            "1709164800 is {leap:?}. The conversion shifts the epoch to 0000-03-01 so \
             the leap day falls at the END of the year and the month lengths become a \
             closed form, then recovers the year with era arithmetic \
             (`doe/36524`, `doe/146096`). MUTATION: drop the century terms and the \
             400-year rule goes with them — every date after 2000-02-29 slips a day"
        ),
    );

    let crossed = capture::civil_from_unix(84_600, 3600);
    c.record(
        "c4.a_zone_offset_carries_the_civil_day_over_midnight",
        crossed == (1970, 1, 2, 0, 30, 0),
        format!(
            "23:30 UTC plus an hour is {crossed:?} — the next day at 00:30, not hour \
             24 of the same one. The offset is added BEFORE the day/second-of-day \
             split, which is the only arrangement that carries. MUTATION: apply it to \
             the hour field afterwards and a screenshot taken late in the evening is \
             filed under yesterday. `local_offset_seconds()` currently returns \
             {} — the recorded deviation is the clock source, not this arithmetic, \
             which is why it is one function away",
            capture::local_offset_seconds()
        ),
    );

    // -- c7: the composition F2 actually calls --------------------------------
    let now = capture::local_civil_now();
    let composed = capture::filename_stem(now.0, now.1, now.2, now.3, now.4, now.5);
    c.record(
        "c7.the_live_path_composes_the_clock_with_the_pattern",
        vanilla_shaped(&format!("{composed}.png")),
        format!(
            "`local_civil_now()` is {now:?}, which `filename_stem` renders as \
             {composed:?} — the same shape `grab` produced in b3, reached from the \
             system clock rather than from a fixture. c1 pins the pattern against a \
             literal; this pins that the live path is wired to it"
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape check must be able to reject, or b3 and c7 prove nothing.
    #[test]
    fn the_shape_check_rejects_what_it_should() {
        assert!(vanilla_shaped("2026-07-28_09.04.05.png"));
        assert!(vanilla_shaped("2026-07-28_09.04.05_2.png"));
        assert!(vanilla_shaped("2026-07-28_09.04.05_17.png"));
        // The symmetric transcription c6 is about.
        assert!(!vanilla_shaped("2026-07-28_09:04:05.png"));
        // A symmetric-the-other-way one, and unpadded fields.
        assert!(!vanilla_shaped("2026.07.28_09.04.05.png"));
        assert!(!vanilla_shaped("2026-7-28_09.04.05.png"));
        assert!(!vanilla_shaped("2026-07-28_09.04.05"));
        assert!(!vanilla_shaped("2026-07-28_09.04.05_x.png"));
        assert!(!vanilla_shaped("screenshot.png"));
    }

    /// `a2` compares against this permutation; it must actually permute.
    #[test]
    fn swap_rb_exchanges_only_the_colour_ends() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(swap_rb(&src), vec![3, 2, 1, 4, 7, 6, 5, 8]);
        // And it is an involution, which is what makes "swapped twice" the
        // double-apply mutation a4 names.
        assert_eq!(swap_rb(&swap_rb(&src)), src.to_vec());
    }

    /// The clear `a1` grades has to be strictly ordered with room to spare, or
    /// the +20 margins could not be met however correct the swizzle was.
    #[test]
    fn the_graded_clear_is_strictly_ordered() {
        assert!(CLEAR_ORDERED[0] > CLEAR_ORDERED[1] + 0.2);
        assert!(CLEAR_ORDERED[1] > CLEAR_ORDERED[2] + 0.2);
        assert_eq!(CLEAR_ORDERED[3], 1.0);
        // And the translucent partner differs only in alpha.
        assert_eq!(CLEAR_ORDERED[..3], CLEAR_TRANSLUCENT[..3]);
    }

    /// The chart `a7` reads the row order against must sit wholly in the top
    /// half — a rect that straddled the middle could not distinguish a flip.
    #[test]
    fn the_chart_sits_in_the_top_left_quadrant() {
        assert!(CHART_ORIGIN[1] + CHART_SIZE[1] < H as f32 / 2.0);
        assert!(CHART_ORIGIN[0] + CHART_SIZE[0] < W as f32 / 2.0);
        // And the clear probe must be outside it, or a1 would grade chart pixels.
        assert!(CLEAR_PROBE.0 as f32 > CHART_ORIGIN[0] + CHART_SIZE[0]);
        assert!(CLEAR_PROBE.1 as f32 > CHART_ORIGIN[1] + CHART_SIZE[1]);
    }
}
