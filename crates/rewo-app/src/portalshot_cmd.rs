//! `rewo portalshot --check` — the end-portal shader oracle (M32b).
//!
//! M32 shipped the end portal / gateway pass and recorded an honest gap: its
//! witnesses graded the pass's *inputs* and geometry, never its output. This
//! renders the real pipeline offscreen, with Vulkan validation on, and reads
//! the pixels back.
//!
//! # Why an exact prediction is possible at all
//!
//! The fragment shader is
//!
//! ```glsl
//! vec3 color = textureProj(Sampler0, texProj0).rgb * COLORS[0];
//! for (int i = 0; i < layers; i++)
//!     color += textureProj(Sampler1, texProj0 * end_portal_layer(i + 1)).rgb * COLORS[i];
//! ```
//!
//! Two independent handles come out of that.
//!
//! **Uniform textures collapse the matrices.** If both samplers hold a single
//! constant colour, every `textureProj` returns that constant whatever the
//! fifteen layer matrices do, and the result is exactly
//!
//! ```text
//! color = sky * COLORS[0] + portal * sum(COLORS[0..layers])
//! ```
//!
//! which the CPU can compute without reproducing a single matrix. That pins the
//! sixteen constants, the layer count, the fact that `COLORS[0]` is applied
//! **twice** (once to the base sample and again as `i = 0`), and that the
//! layers accumulate additively rather than compositing.
//!
//! **One layer isolates one sample, and then the matrix is observable.**
//! `textureProj` divides by `p.w`, and every layer matrix's column 3 is
//! `(0, 0, 0, 1)`, so that divide cancels; the composed column 0 also has a
//! zero depth term. The sampled `u` is therefore an *affine function of the
//! screen UV alone* — a few lines of Rust. Rendering a left-red / right-blue
//! texture through a single layer makes the predicted half directly visible,
//! and the transposed reading of the same matrix predicts a different half at
//! some pixels. See [`check_layer_matrix`]; that is the witness M32's
//! near-miss earned.
//!
//! Both textures are **synthetic**, never the jar's. An oracle whose expected
//! value depends on an asset's contents is testing the asset; the jar's own
//! textures are already pinned by `blockentityshot`'s `p3`.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use rewo_data::{assets, DataPaths};
use rewo_gpu::end_portal::{PortalDraw, PortalImage, PortalVertex, GATEWAY_LAYERS, PORTAL_LAYERS};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, SkyMode, WorldRenderer};
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

/// A witness that stops running is a failure, not a quieter pass.
const EXPECTED_WITNESSES: usize = 12;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 128;
const H: u32 = 128;

/// The sixteen `COLORS` from `core/rendertype_end_portal.fsh`, transcribed
/// **here as well as in the shader**, from the decompiled source. The oracle's
/// expectation must not be read out of the thing it is grading.
const COLORS: [[f32; 3]; 16] = [
    [0.022087, 0.098399, 0.110818],
    [0.011892, 0.095924, 0.089485],
    [0.027636, 0.101689, 0.100326],
    [0.046564, 0.109883, 0.114838],
    [0.064901, 0.117696, 0.097189],
    [0.063761, 0.086895, 0.123646],
    [0.084817, 0.111994, 0.166380],
    [0.097489, 0.154120, 0.091064],
    [0.106152, 0.131144, 0.195191],
    [0.097721, 0.110188, 0.187229],
    [0.133516, 0.138278, 0.148582],
    [0.070006, 0.243332, 0.235792],
    [0.196766, 0.142899, 0.214696],
    [0.047281, 0.315338, 0.321970],
    [0.204675, 0.390010, 0.302066],
    [0.080955, 0.314821, 0.661491],
];

/// Channel tolerance, in stored 0..255 units. The differences this gate must
/// resolve are far larger — the unit tests below assert as much.
const CHANNEL_TOL: f32 = 1.5;

/// How far a predicted `u` must sit from a texture seam (0, 0.5, 1) before its
/// pixel is graded. One texel of the 16-wide test texture is 0.0625, so
/// bilinear blending reaches ±0.031; 0.12 clears it with room to spare.
const SEAM_MARGIN: f32 = 0.12;

#[derive(ClapArgs, Debug)]
pub struct PortalshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// The sole, explicit opt-out from Vulkan validation.
    #[arg(long, default_value_t = false)]
    pub no_validation: bool,
    /// Dump each rendered frame, for eyeballing a failure.
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
            "[portalshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// -- colour space -------------------------------------------------------------
// Both textures and the offscreen attachment are `R8G8B8A8_SRGB`, so the
// hardware decodes on sample and re-encodes on store. The shader's arithmetic
// happens in linear between the two, and the prediction has to model both ends.

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// What the shader must produce, in **stored sRGB bytes**, for uniform inputs.
fn expected_uniform(sky: [u8; 3], portal: [u8; 3], layers: usize) -> [f32; 3] {
    let mut out = [0f32; 3];
    for (ch, o) in out.iter_mut().enumerate() {
        let s = srgb_to_linear(sky[ch] as f32 / 255.0);
        let p = srgb_to_linear(portal[ch] as f32 / 255.0);
        let mut acc = s * COLORS[0][ch];
        for c in COLORS.iter().take(layers) {
            acc += p * c[ch];
        }
        *o = linear_to_srgb(acc.clamp(0.0, 1.0)) * 255.0;
    }
    out
}

// -- rendering rig ------------------------------------------------------------

/// A uniform texture, as one sRGB byte triple over `side * side` texels.
fn flat(rgb: [u8; 3], side: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((side * side * 4) as usize);
    for _ in 0..side * side {
        v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    v
}

/// A slab facing the camera, large enough to cover the whole viewport at the
/// distance used here — every graded pixel must actually be portal. `x_off`
/// slides it in world space while keeping that coverage.
fn facing_quad(z: f32, x_off: f32) -> Vec<PortalVertex> {
    let v = |x: f32, y: f32| PortalVertex {
        pos: [x + x_off, y, z],
    };
    vec![
        v(-4.0, -4.0),
        v(4.0, -4.0),
        v(4.0, 4.0),
        v(-4.0, -4.0),
        v(4.0, 4.0),
        v(-4.0, 4.0),
    ]
}

/// A view-projection at the origin looking down −Z, optionally rolled about
/// its own view axis by tilting the up vector.
fn view_proj(roll_x: f32) -> [[f32; 4]; 4] {
    let proj = perspective_reverse_z(70f32.to_radians(), W as f32 / H as f32, 0.05);
    let view = glam::Mat4::look_at_rh(
        glam::Vec3::ZERO,
        glam::Vec3::new(0.0, 0.0, -1.0),
        glam::Vec3::new(roll_x, 1.0, 0.0).normalize(),
    );
    (glam::Mat4::from_cols_array_2d(&proj) * view).to_cols_array_2d()
}

/// Render one portal through the production `WorldRenderer` path and read the
/// pixels back. The sky pass is off and no columns are set, so everything in
/// the frame that is not the clear colour came out of the portal shader.
#[allow(clippy::too_many_arguments)]
fn render_portal(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    sky: (&[u8], u32),
    portal: (&[u8], u32),
    layers: i32,
    game_time: i64,
    vp: [[f32; 4]; 4],
    x_off: f32,
) -> Result<Vec<u8>, String> {
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.set_camera([0.0, 0.0, 0.0]);
    wr.set_sky_mode(SkyMode::None);
    wr.init_end_portal(
        gpu,
        &PortalImage {
            rgba: sky.0,
            w: sky.1,
            h: sky.1,
        },
        &PortalImage {
            rgba: portal.0,
            w: portal.1,
            h: portal.1,
        },
    )?;
    wr.set_end_portals(
        gpu,
        &[PortalDraw {
            verts: facing_quad(-2.0, x_off),
            layers,
        }],
        game_time,
    )?;
    off.render(gpu, Some((&mut wr, vp)), draw, CLEAR)?;
    let img = off.read_rgba(gpu)?;
    wr.destroy(gpu);
    Ok(img)
}

fn center_px(img: &[u8]) -> [f32; 3] {
    let i = (((H / 2) * W + W / 2) * 4) as usize;
    [img[i] as f32, img[i + 1] as f32, img[i + 2] as f32]
}

fn max_err(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
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

fn dump(args: &PortalshotArgs, off: &mut Offscreen, gpu: &Gpu, name: &str) {
    if let Some(dir) = &args.out_dir {
        let _ = off.save_png(gpu, &dir.join(format!("{name}.png")));
    }
}

/// An overlay parked far offscreen — the frame-time chart must not paint over
/// anything this gate reads.
fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

// -- entry point --------------------------------------------------------------

pub fn run(args: PortalshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

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
    println!("[portalshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "portalshot check: Vulkan validation requested but not active — install \
             the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass --no-validation"
                .into(),
        );
    }
    run_check(&mut gpu, &baked, &args)
}

fn run_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    args: &PortalshotArgs,
) -> Result<(), String> {
    let mut off = Offscreen::new(gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    let rendered = check_uniform_sum(&mut c, gpu, &mut off, baked, &draw, args)
        .and_then(|()| check_screen_space(&mut c, gpu, &mut off, baked, &draw, args))
        .and_then(|()| check_layer_matrix(&mut c, gpu, &mut off, baked, &draw, args));
    // Before the device goes away, and on the error path too — a leaked
    // `Offscreen` is a fistful of `VUID-vkDestroyDevice-device-05137`s, and
    // this gate runs with validation ON.
    off.destroy(gpu);
    rendered?;

    println!(
        "[portalshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
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
            "portalshot observed {} witnesses, expected {EXPECTED_WITNESSES} — a \
             witness that stops running is a failure, not a quieter pass",
            c.witnessed
        ));
    }
    println!("[portalshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// -- Part A: the analytic sum -------------------------------------------------

const SKY: [u8; 3] = [180, 180, 180];
const POR: [u8; 3] = [200, 120, 60];

fn check_uniform_sum(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    args: &PortalshotArgs,
) -> Result<(), String> {
    let sky = flat(SKY, 4);
    let por = flat(POR, 4);
    let black = flat([0, 0, 0], 4);
    let vp = view_proj(0.0);

    let img = render_portal(
        gpu,
        off,
        baked,
        draw,
        (&sky, 4),
        (&por, 4),
        PORTAL_LAYERS,
        0,
        vp,
        0.0,
    )?;
    dump(args, off, gpu, "uniform-portal");
    let got = center_px(&img);
    let want = expected_uniform(SKY, POR, PORTAL_LAYERS as usize);
    c.record(
        "v1.the_uniform_sum_is_exact",
        max_err(got, want) <= CHANNEL_TOL,
        format!(
            "read {got:?}, computed {want:?}, max channel error {:.2}. With uniform \
             textures every textureProj returns the same constant whatever the layer \
             matrices do, so this is `sky*COLORS[0] + portal*sum(COLORS[0..15])` \
             computed on the CPU from an INDEPENDENT transcription of the table — it \
             pins the sixteen constants, the layer count and the additive \
             accumulation without reproducing a single matrix",
            max_err(got, want)
        ),
    );

    // Guards: the match above must not be two black images agreeing.
    let dark = render_portal(
        gpu,
        off,
        baked,
        draw,
        (&sky, 4),
        (&black, 4),
        PORTAL_LAYERS,
        0,
        vp,
        0.0,
    )?;
    let dark_px = center_px(&dark);
    c.record(
        "v2.the_layer_sampler_actually_contributes",
        dark_px.iter().sum::<f32>() + 20.0 < got.iter().sum::<f32>()
            && max_err(
                dark_px,
                expected_uniform(SKY, [0, 0, 0], PORTAL_LAYERS as usize),
            ) <= CHANNEL_TOL,
        format!(
            "a black Sampler1 gives {dark_px:?} against {got:?} — the loop is \
             genuinely reading that texture, and what remains is exactly the base term"
        ),
    );

    let no_sky = render_portal(
        gpu,
        off,
        baked,
        draw,
        (&black, 4),
        (&por, 4),
        PORTAL_LAYERS,
        0,
        vp,
        0.0,
    )?;
    let no_sky_px = center_px(&no_sky);
    c.record(
        "v3.the_sky_sampler_is_the_base_term",
        no_sky_px.iter().sum::<f32>() + 5.0 < got.iter().sum::<f32>()
            && max_err(
                no_sky_px,
                expected_uniform([0, 0, 0], POR, PORTAL_LAYERS as usize),
            ) <= CHANNEL_TOL,
        format!(
            "a black Sampler0 gives {no_sky_px:?} — Sampler0 is `end_sky.png` and it \
             enters ONCE, before the loop, times COLORS[0]"
        ),
    );

    let gate = render_portal(
        gpu,
        off,
        baked,
        draw,
        (&sky, 4),
        (&por, 4),
        GATEWAY_LAYERS,
        0,
        vp,
        0.0,
    )?;
    dump(args, off, gpu, "uniform-gateway");
    let gate_px = center_px(&gate);
    let want_gate = expected_uniform(SKY, POR, GATEWAY_LAYERS as usize);
    c.record(
        "v4.a_gateway_is_exactly_one_more_layer_than_a_portal",
        max_err(gate_px, want_gate) <= CHANNEL_TOL && gate_px != got,
        format!(
            "16 layers read {gate_px:?} against 15's {got:?}, computed {want_gate:?} \
             (error {:.2}) — the extra term is portal*COLORS[15], the single \
             difference between vanilla's two pipelines",
            max_err(gate_px, want_gate)
        ),
    );

    let alpha = img[(((H / 2) * W + W / 2) * 4 + 3) as usize];
    c.record(
        "v5.the_output_is_opaque",
        alpha == 255,
        format!(
            "alpha {alpha} — the shader writes `vec4(color, 1.0)` and the pipeline \
             sets `blend_enable(false)`, so a portal OVERWRITES. Its glow comes from \
             the shader's own accumulation, not from additive blending"
        ),
    );

    // One layer: the sum is `sky*C0 + portal*C0`. A loop starting at COLORS[1],
    // or a dropped base term, misses this by a wide margin.
    let one = render_portal(gpu, off, baked, draw, (&sky, 4), (&por, 4), 1, 0, vp, 0.0)?;
    let one_px = center_px(&one);
    let want_one = expected_uniform(SKY, POR, 1);
    c.record(
        "v6.colors_zero_is_applied_to_both_the_base_and_the_first_layer",
        max_err(one_px, want_one) <= CHANNEL_TOL,
        format!(
            "at one layer: read {one_px:?}, computed {want_one:?} (error {:.2}). The \
             base sample uses COLORS[0] and so does `i = 0`, so that constant enters \
             TWICE against different samplers — the layer index runs 1..N while the \
             colour index runs 0..N-1, off by one on purpose",
            max_err(one_px, want_one)
        ),
    );
    Ok(())
}

// -- Part B: the sample is welded to the screen -------------------------------

/// How much a re-projection may disturb an image that should be identical.
/// `texProj0` is a vertex attribute, so different clip coordinates for the same
/// screen point interpolate to the same value only up to float rounding: a
/// handful of channels land a quantisation step apart.
///
/// The **delta** is the load-bearing half. A count budget alone could be
/// satisfied by a few catastrophically wrong pixels; "no channel moved by more
/// than one step" cannot be satisfied by anything that genuinely resampled, and
/// `v8` shows a real change reaching 40. Measured here: 175 and 51 bytes of
/// 65,536, both at delta 1.
const REPROJECTION_BYTES: usize = (W * H * 4) as usize / 200;
const REPROJECTION_DELTA: u8 = 2;

/// The defining property of this render type: it samples in **screen** space.
///
/// `texProj0` is `projection_from_position(gl_Position)`, so the sampled texel
/// is a function of the pixel's position on screen and nothing else. For a
/// portal that covers the viewport, that means sliding the quad through the
/// world — or rolling the camera — must leave the image *identical*. The
/// starfield is welded to the screen and appears to swim against the world.
///
/// This is the inverse of the obvious guess, and it is what makes the mistake
/// the mesh's unused UVs invited detectable: a model-space sampler would have
/// moved its pattern with the geometry.
fn check_screen_space(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    args: &PortalshotArgs,
) -> Result<(), String> {
    // A 2x2 checker: the sampled value swings hard with position, so anything
    // that genuinely moved the sample would be unmissable.
    let tex: Vec<u8> = vec![
        255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255,
    ];
    let shot = |gpu: &mut Gpu, off: &mut Offscreen, time, vp, x_off| {
        render_portal(
            gpu,
            off,
            baked,
            draw,
            (&tex, 2),
            (&tex, 2),
            PORTAL_LAYERS,
            time,
            vp,
            x_off,
        )
    };

    let base = shot(gpu, off, 0, view_proj(0.0), 0.0)?;
    dump(args, off, gpu, "screen-base");
    let moved = shot(gpu, off, 0, view_proj(0.0), 1.5)?;
    dump(args, off, gpu, "screen-moved");
    let rolled = shot(gpu, off, 0, view_proj(0.35), 0.0)?;
    dump(args, off, gpu, "screen-rolled");

    let (dm, dr) = (
        bytes_differing(&base, &moved),
        bytes_differing(&base, &rolled),
    );
    let (em, er) = (
        largest_byte_delta(&base, &moved),
        largest_byte_delta(&base, &rolled),
    );
    c.record(
        "v7.the_sample_is_welded_to_the_screen_not_the_model",
        dm <= REPROJECTION_BYTES
            && dr <= REPROJECTION_BYTES
            && em <= REPROJECTION_DELTA
            && er <= REPROJECTION_DELTA,
        format!(
            "sliding the quad 1.5 blocks through the world changed {dm} of {} bytes \
             (largest delta {em}), and rolling the camera about its view axis changed \
             {dr} (largest delta {er}) — both within re-projection rounding. \
             `texProj0` is `projection_from_position(gl_Position)`, so the sampled \
             texel is a function of SCREEN position alone; the starfield stays put \
             while the world moves under it. A model-space sampler would have \
             dragged its pattern along with the geometry",
            base.len()
        ),
    );

    // The sensitivity partner: the very same comparison must be able to see a
    // real change, or v7 would be a tautology about a comparison that never
    // fires.
    let later = shot(gpu, off, 5_000, view_proj(0.0), 0.0)?;
    c.record(
        "v8.the_starfield_scrolls_with_the_world_clock",
        bytes_differing(&base, &later) > (W * H) as usize / 20
            && largest_byte_delta(&base, &later) > 8 * REPROJECTION_DELTA,
        format!(
            "advancing the clock changed {} of {} bytes (largest delta {}) with the \
             camera and the portal both still — each layer's translate carries \
             `(2 + layer/1.5) * (GameTime * 1.5)` in its y term. This is the same \
             byte comparison v7 used, so v7's near-identity is a measured result, \
             not a blind spot",
            bytes_differing(&base, &later),
            base.len(),
            largest_byte_delta(&base, &later)
        ),
    );
    Ok(())
}

// -- Part C: the matrix convention --------------------------------------------

/// The witness M32's near-miss earned.
///
/// GLSL's `mat4(...)` is **column-major**, and vanilla's source is GLSL, so
/// `translate`'s `17/layer` sits at `m[0][3]`. It reaches the coordinate
/// because the sampling is `texProj0 * matrix`, a **row-vector** multiply.
/// Rewriting the literal into the slots it "looks like" it belongs in
/// transposes it, and still produces a perfectly plausible swirl.
///
/// With one layer the colour is a single `textureProj`, so the sampled point
/// can be predicted here and compared. The prediction is built from the
/// decompiled source independently of the shipped shader.
fn check_layer_matrix(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    baked: &assets::BakedAssets,
    draw: &OverlayDraw,
    args: &PortalshotArgs,
) -> Result<(), String> {
    // Left half red, right half blue: the sampled `u` alone decides the colour.
    let side = 16u32;
    let mut tex = Vec::new();
    for _y in 0..side {
        for x in 0..side {
            let px = if x < side / 2 {
                [255u8, 0, 0]
            } else {
                [0, 0, 255]
            };
            tex.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    // A black sky kills the base term, leaving exactly one sample in the frame.
    let black = flat([0, 0, 0], 2);
    let vp = view_proj(0.0);

    let one = render_portal(gpu, off, baked, draw, (&black, 2), (&tex, side), 1, 0, vp, 0.0)?;
    dump(args, off, gpu, "matrix-one-layer");

    let m = layer_matrix(1.0, 0.0);
    let mt = transpose(&m);

    // Grade a grid of pixel centres. `u = dot((screen_u, screen_v, z/w, 1),
    // column 0)` — the depth term drops because the composed `m[0][2]` is zero,
    // and the projective divide cancels because column 3 is `(0,0,0,1)`.
    let mut graded = 0usize;
    let mut agreed = 0usize;
    let mut discriminating = 0usize;
    let mut misses = Vec::new();
    for gy in 1..8u32 {
        for gx in 1..8u32 {
            let (px, py) = (gx * W / 8, gy * H / 8);
            // The viewport is y-flipped, so screen v = 1 - pixel_y / height.
            let su = (px as f32 + 0.5) / W as f32;
            let sv = 1.0 - (py as f32 + 0.5) / H as f32;
            let want_u = sample_u(&m, su, sv);
            let trans_u = sample_u(&mt, su, sv);
            if half_of(want_u) != half_of(trans_u) {
                discriminating += 1;
            }
            // Skip pixels whose predicted sample sits on a seam, where bilinear
            // filtering makes "which half" genuinely ambiguous.
            if !clear_of_seam(want_u) {
                continue;
            }
            graded += 1;
            let i = ((py * W + px) * 4) as usize;
            let is_red = one[i] > one[i + 2];
            let want_red = half_of(want_u);
            if is_red == want_red {
                agreed += 1;
            } else if misses.len() < 4 {
                misses.push(format!(
                    "px({px},{py}) u={:.3} want {} got {}",
                    want_u.rem_euclid(1.0),
                    if want_red { "R" } else { "B" },
                    if is_red { "R" } else { "B" }
                ));
            }
        }
    }

    c.record(
        "v9.the_transposed_matrix_would_be_observably_different",
        discriminating >= 4,
        format!(
            "the column-major and transposed readings predict different halves at \
             {discriminating} of 49 sampled pixels — without this the witness below \
             would prove nothing, because a convention that made no difference \
             anywhere could not be got wrong"
        ),
    );
    c.record(
        "v10.the_render_matches_the_column_major_reading",
        graded >= 20 && agreed == graded,
        format!(
            "{agreed}/{graded} graded pixels match the prediction{}. GLSL's `mat4` \
             constructor is COLUMN-major and vanilla's source is GLSL, so `17/layer` \
             sits at m[0][3] and reaches the coordinate through the ROW-vector \
             multiply `texProj0 * matrix`",
            if misses.is_empty() {
                String::new()
            } else {
                format!(" (first misses: {})", misses.join("; "))
            }
        ),
    );

    // A prediction that agreed with a black clear would prove nothing either.
    let covered = one.chunks_exact(4).filter(|p| p[0] > 8 || p[2] > 8).count();
    c.record(
        "v11.the_portal_actually_covered_the_pixels_read",
        covered > (W * H) as usize / 4,
        format!(
            "{covered} of {} pixels carry portal colour — the graded pixels are \
             portal output, not clear colour",
            W * H
        ),
    );

    // And the single-layer isolation must be real.
    let two = render_portal(gpu, off, baked, draw, (&black, 2), (&tex, side), 2, 0, vp, 0.0)?;
    c.record(
        "v12.one_layer_is_genuinely_one_sample",
        bytes_differing(&one, &two) > (W * H) as usize / 20,
        format!(
            "adding a second layer changed {} of {} bytes — so the render graded \
             above really was a single textureProj, which is what makes the matrix \
             observable at all",
            bytes_differing(&one, &two),
            one.len()
        ),
    );
    Ok(())
}

fn half_of(u: f32) -> bool {
    u.rem_euclid(1.0) < 0.5
}

fn clear_of_seam(u: f32) -> bool {
    let u = u.rem_euclid(1.0);
    u > SEAM_MARGIN && (u - 0.5).abs() > SEAM_MARGIN && u < 1.0 - SEAM_MARGIN
}

/// The `u` a screen point samples at, through one layer matrix.
///
/// Row-vector multiply: `q[j] = dot(v, column j)`, then the projective divide.
fn sample_u(m: &[[f32; 4]; 4], su: f32, sv: f32) -> f32 {
    let v = [su, sv, 0.0, 1.0];
    let dot = |col: usize| (0..4).map(|i| v[i] * m[col][i]).sum::<f32>();
    dot(0) / dot(3)
}

/// `end_portal_layer(layer)`, transcribed from the decompiled `.fsh`
/// **independently of the shader Rewo ships**, in GLSL's column-major
/// convention: `m[column][row]`.
fn layer_matrix(layer: f32, game_time: f32) -> [[f32; 4]; 4] {
    // `mat4(a, b, c, d, ...)` fills COLUMN 0 with (a, b, c, d) — so the
    // "translation" lands in row 3 of columns 0 and 1, not in column 3.
    let mut translate = identity();
    translate[0][3] = 17.0 / layer;
    translate[1][3] = (2.0 + layer / 1.5) * (game_time * 1.5);

    let ang = ((layer * layer * 4321.0 + layer * 9.0) * 2.0).to_radians();
    let (s, co) = ang.sin_cos();
    // `mat2(cos, -sin, sin, cos)` — column 0 is (cos, -sin).
    let rot = [[co, -s], [s, co]];
    // `mat2(k)` is k on the diagonal.
    let k = (4.5 - layer / 4.0) * 2.0;
    let sc = [[k, 0.0], [0.0, k]];
    let mut sr = [[0f32; 2]; 2];
    for j in 0..2 {
        for i in 0..2 {
            sr[j][i] = (0..2).map(|kk| sc[kk][i] * rot[j][kk]).sum();
        }
    }
    // `mat4(mat2)` embeds the 2x2 and leaves 1 on the remaining diagonal.
    let mut srm = identity();
    for j in 0..2 {
        for i in 0..2 {
            srm[j][i] = sr[j][i];
        }
    }
    // `SCALE_TRANSLATE`, likewise column-major.
    let mut st = identity();
    st[0][0] = 0.5;
    st[1][1] = 0.5;
    st[0][3] = 0.25;
    st[1][3] = 0.25;

    mul4(&mul4(&srm, &translate), &st)
}

fn identity() -> [[f32; 4]; 4] {
    let mut m = [[0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// Column-major `a * b`: `(a*b)[col][row] = sum_k a[k][row] * b[col][k]`.
fn mul4(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0f32; 4]; 4];
    for j in 0..4 {
        for i in 0..4 {
            o[j][i] = (0..4).map(|k| a[k][i] * b[j][k]).sum();
        }
    }
    o
}

fn transpose(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0f32; 4]; 4];
    for j in 0..4 {
        for i in 0..4 {
            o[j][i] = m[i][j];
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_colours_table_is_transcribed_whole() {
        assert_eq!(COLORS.len(), 16);
        assert!((COLORS[0][0] - 0.022087).abs() < 1e-9);
        assert!((COLORS[15][2] - 0.661491).abs() < 1e-9);
    }

    /// The gateway's sixteenth layer must be worth more than the tolerance, or
    /// `v4` would pass on a portal.
    #[test]
    fn the_sixteenth_layer_is_worth_more_than_the_tolerance() {
        let a = expected_uniform(SKY, POR, 15);
        let b = expected_uniform(SKY, POR, 16);
        assert!(
            max_err(a, b) > CHANNEL_TOL * 3.0,
            "15 vs 16 layers: {a:?} vs {b:?}"
        );
    }

    /// Likewise the base term: `v2` / `v3` must be able to see it go missing.
    #[test]
    fn the_base_term_is_worth_more_than_the_tolerance() {
        let with = expected_uniform(SKY, POR, 15);
        let without = expected_uniform([0, 0, 0], POR, 15);
        assert!(max_err(with, without) > CHANNEL_TOL * 3.0);
    }

    /// The projective divide must cancel and the depth term must drop, or
    /// [`sample_u`]'s two-argument form would be wrong.
    #[test]
    fn the_layer_matrix_leaves_w_and_z_out_of_the_u_coordinate() {
        for layer in [1.0f32, 2.0, 7.0, 15.0] {
            let m = layer_matrix(layer, 0.37);
            assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0], "column 3 at layer {layer}");
            assert!(
                m[0][2].abs() < 1e-6,
                "depth term at layer {layer}: {}",
                m[0][2]
            );
        }
    }

    /// The translate really does live at `[0][3]`, and the matrix is not its
    /// own transpose — else `v9` / `v10` would be vacuous.
    #[test]
    fn the_translate_sits_in_the_column_major_slot() {
        let m = layer_matrix(1.0, 0.0);
        // 17 through SCALE_TRANSLATE's 0.5, plus its own 0.25.
        assert!((m[0][3] - 8.75).abs() < 1e-4, "m[0][3] = {}", m[0][3]);
        assert_ne!(m, transpose(&m));
    }

    #[test]
    fn seam_rejection_keeps_only_unambiguous_samples() {
        assert!(!clear_of_seam(0.5));
        assert!(!clear_of_seam(0.02));
        assert!(!clear_of_seam(0.99));
        assert!(clear_of_seam(0.25));
        assert!(clear_of_seam(0.75));
        // `rem_euclid` first: the raw `u` runs well past 1.
        assert!(clear_of_seam(9.25));
    }
}
