//! `rewo lightmapshot` — serverless headless verification of the production
//! world lightmap (M13).
//!
//! The M12 skyshot gate verifies the *sky*; this gate verifies the *lightmap*
//! the terrain fragment shader (`shaders/lightmap.glsl`, included by both
//! `world.frag` and `water.frag`) computes from a vertex's packed
//! `(block, sky)` levels.
//!
//! This is a **TERRAIN + WATER + ENTITY** matrix. The terrain/water cases
//! render the same large white quad — as opaque terrain geometry or as
//! translucent water geometry — through the real `WorldRenderer`, then compare
//! the GPU readback against an independent CPU lightmap. It is a
//! GPU-vs-independent-CPU oracle, never a "looks right" proxy. The final case
//! proves the **entity** light transport: entities don't sample the lightmap
//! texture — the caller hands each `EntityDraw` a per-channel `light: [f32; 3]`
//! (its own eye's lightmap colour), and entity vertex generation multiplies
//! the model's directional face shade by it before the production shader. That case renders one capsule lit by
//! the distinctive warm block-light colour and checks that the readback's
//! normalized RGB ratios match the CPU light's — the scalar per-face shade
//! cancels under the normalization, so only the `[f32; 3]` transport is tested.
//!
//! The shared oracle per case:
//!
//! - Render a single large top-facing **white** quad (packed `block`/`sky`,
//!   white vertex colour) with a stable straight-down camera, fog pushed far
//!   away and the sky/fog painted black. Because the texture texel and the
//!   vertex colour are both 1.0, the fragment's `c.rgb * v_color * lm` reduces
//!   to exactly the lightmap `lm`; the `R8G8B8A8_SRGB` attachment then encodes
//!   it linear→sRGB on store. For the water pass the texel alpha is 255, so the
//!   blend collapses to the opaque result (the write mask is RGB only).
//! - Sample an interior centre patch → the actual sRGB bytes.
//! - Independently compute the expected lightmap from
//!   `rewo_world::lightmap::sample(block, sky, ..)` with the *matching* state,
//!   encode linear→sRGB with the standard piecewise formula, round to bytes.
//! - Require each channel within ±2, and assert the case-specific property
//!   (warm tint, block-factor response, gamma ramp, night-vision lift, the
//!   black-texel NaN store, the darkness terms, water==opaque, entity
//!   `[f32; 3]` light transport).

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_gpu::entities::{EntityDraw, EntityModelKind, MobTextures};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldLightmapState, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::{pack_layer, MeshVertex};
use rewo_world::lightmap::{darkness_lightmap, mth_cos, sample, LightmapState};

use crate::stats::OverlayRing;

/// Black clear — every case paints its own sky/fog black anyway, and the quad
/// covers the sampled patch.
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// The single synthetic texture layer is `TEX²` opaque white (alpha 255).
const TEX: u32 = 16;
/// Offscreen resolution.
const W: u32 = 128;
const H: u32 = 128;
/// Per-channel tolerance (sRGB bytes).
const CHANNEL_TOL: i32 = 2;

#[derive(ClapArgs)]
pub struct LightmapshotArgs {
    /// Assert the lightmap properties and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Optional: also dump one PNG per case here (eyeball artifacts).
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Bypass Vulkan validation layers. Otherwise the gate requests them in
    /// every build and, under `--check`, fails if they don't activate.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: LightmapshotArgs) -> Result<(), String> {
    // The permanent gate runs with Vulkan validation ON in every build (debug
    // AND release) unless explicitly opted out. Mirrors skyshot.
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;

    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[lightmapshot] Vulkan validation: {status}");

    // Under `--check`, requesting validation but not getting it must fail
    // loudly rather than grade a frame with validation silently off.
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "lightmapshot check: Vulkan validation requested but not active — \
                install the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass \
                --no-validation to bypass"
                .into(),
        );
    }

    run_check(&mut gpu, &args)
}

fn run_check(gpu: &mut Gpu, args: &LightmapshotArgs) -> Result<(), String> {
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let mut off = Offscreen::new(gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);

    // One synthetic 16×16 opaque-white RGBA layer — no jar, no server.
    let white: Vec<u8> = vec![255u8; (TEX * TEX * 4) as usize];
    let layers = [white];

    let mut wr = WorldRenderer::new(gpu, off.format, TEX, &layers)?;

    // Stable straight-down camera: above the column centre looking −Y, up +Z.
    let eye = Vec3::new(8.0, 80.0, 8.0);
    wr.set_camera([eye.x, eye.y, eye.z]);
    let view = Mat4::look_to_rh(eye, Vec3::new(0.0, -1.0, 0.0), Vec3::Z);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let view_proj = (proj * view).to_cols_array_2d();

    // Fog far away, sky + fog black — the quad covers the sampled patch anyway.
    wr.set_fog(1.0e6, 1.0e6 + 1.0);
    wr.set_sky_tint([0.0; 3], [0.0; 3]);

    // Run the whole matrix, accumulating property failures. GPU errors abort
    // (propagated after teardown); property mismatches accumulate.
    let mut failures: Vec<String> = Vec::new();
    let result = run_cases(
        gpu,
        &mut off,
        &mut wr,
        view_proj,
        &draw,
        args,
        &mut failures,
    );

    // Clean teardown regardless of outcome.
    wr.destroy(gpu);
    off.destroy(gpu);

    // A genuine GPU/render error is fatal; surface it before grading.
    result?;

    if failures.is_empty() {
        if args.check {
            println!("[lightmapshot] CHECK OK — terrain + water + entity lightmap matrix verified");
        } else {
            println!("[lightmapshot] PASS — terrain + water + entity lightmap matrix verified (assertion mode off)");
        }
        Ok(())
    } else {
        for f in &failures {
            println!("[lightmapshot] FAIL {f}");
        }
        if args.check {
            Err(format!(
                "lightmapshot check: {} property failures",
                failures.len()
            ))
        } else {
            Ok(())
        }
    }
}

/// The full terrain + water case matrix. Each case renders through the real
/// world path and compares against the independent CPU lightmap.
#[allow(clippy::too_many_arguments)]
fn run_cases(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    view_proj: [[f32; 4]; 4],
    draw: &OverlayDraw,
    args: &LightmapshotArgs,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    // --- Case 1: warm block tint. block 7, sky 0, block_factor 1.0, gamma 0.
    // The production shader tints block light 0xFFD88C (warm), so the rendered
    // blue must land far nearer the correct value than the old bluer 0xFFD86C.
    {
        let name = "tint-b7s0-f1";
        let gs = WorldLightmapState {
            block_factor: 1.0,
            ..Default::default()
        };
        let (exp, lm) = expected_srgb(7, 0, &cpu_state(&gs));
        let act = render_quad(gpu, off, wr, view_proj, draw, name, 7, 0, gs, false, args)?;
        compare_case(failures, name, act, exp, lm);

        // Old naive lightmap blue byte (0xFFD86C → 0x6C). Independent of the
        // CPU oracle: require the render is >= 5 bytes closer to the correct
        // blue than to the old one.
        const OLD_BLUE: i32 = 0x6C; // 108
        let correct_b = exp[2] as i32;
        let actual_b = act[2] as i32;
        let closer = (actual_b - OLD_BLUE).abs() - (actual_b - correct_b).abs();
        println!(
            "[lightmapshot] {name}: blue actual {actual_b} correct {correct_b} old {OLD_BLUE} → closer-by {closer}"
        );
        if closer < 5 {
            failures.push(format!(
                "{name}: blue {actual_b} not >=5 closer to correct {correct_b} than old {OLD_BLUE} (closer-by {closer})"
            ));
        }
    }

    // --- Case 2: block-factor response. block 7, sky 0, factor 1.0 vs 1.8.
    // Each CPU-matches; the two must differ measurably (>= 10 in a channel).
    {
        let n1 = "block-f1.0-b7s0";
        let g1 = WorldLightmapState {
            block_factor: 1.0,
            ..Default::default()
        };
        let (e1, lm1) = expected_srgb(7, 0, &cpu_state(&g1));
        let a1 = render_quad(gpu, off, wr, view_proj, draw, n1, 7, 0, g1, false, args)?;
        compare_case(failures, n1, a1, e1, lm1);

        let n2 = "block-f1.8-b7s0";
        let g2 = WorldLightmapState {
            block_factor: 1.8,
            ..Default::default()
        };
        let (e2, lm2) = expected_srgb(7, 0, &cpu_state(&g2));
        let a2 = render_quad(gpu, off, wr, view_proj, draw, n2, 7, 0, g2, false, args)?;
        compare_case(failures, n2, a2, e2, lm2);

        let d = max_delta(a1, a2);
        println!("[lightmapshot] block-factor 1.0 vs 1.8 max channel Δ {d}");
        if d < 10 {
            failures.push(format!(
                "block-factor 1.0 vs 1.8 differ by only {d} (<10, expected a clear response)"
            ));
        }
    }

    // --- Case 3: gamma (brightness_factor) ramp on a mid, non-clamped cell.
    // block 4, sky 4 sits well below 1.0 so notGamma has room to brighten;
    // each CPU-matches and the luma must strictly increase with gamma.
    {
        let steps = [(0.0f32, "gamma0"), (0.5, "gamma.5"), (1.0, "gamma1")];
        let mut lumas = [0f32; 3];
        for (i, (bf, tag)) in steps.iter().enumerate() {
            let name = format!("{tag}-b4s4");
            let gs = WorldLightmapState {
                brightness_factor: *bf,
                ..Default::default()
            };
            let (e, lm) = expected_srgb(4, 4, &cpu_state(&gs));
            let a = render_quad(gpu, off, wr, view_proj, draw, &name, 4, 4, gs, false, args)?;
            compare_case(failures, &name, a, e, lm);
            lumas[i] = luma(a);
        }
        println!(
            "[lightmapshot] gamma luma ramp {:.2} → {:.2} → {:.2}",
            lumas[0], lumas[1], lumas[2]
        );
        if !(lumas[0] < lumas[1] && lumas[1] < lumas[2]) {
            failures.push(format!(
                "gamma luma not strictly increasing: {:.2} {:.2} {:.2}",
                lumas[0], lumas[1], lumas[2]
            ));
        }
    }

    // --- Case 4: night vision lifts a black texel. block 0, sky 0, NV 1,
    // gamma 0 → ambient seeds (0.6,0.6,0.6), so no 0/0 and a definite grey.
    {
        let name = "nv-black-b0s0";
        let gs = WorldLightmapState {
            night_vision_factor: 1.0,
            ..Default::default()
        };
        let (e, lm) = expected_srgb(0, 0, &cpu_state(&gs));
        let a = render_quad(gpu, off, wr, view_proj, draw, name, 0, 0, gs, false, args)?;
        compare_case(failures, name, a, e, lm);
    }

    // --- Case 5: the black-texel NaN store. block 0, sky 0, NV 0, gamma 0.
    // The CPU lightmap is a genuine 0/0 NaN (not byte-convertible), so this
    // case does NOT compare to a CPU expected — it pins what the GPU's RGBA8
    // store does with the NaN by readback: every channel must be ~black (<= 1).
    {
        let name = "black-nan-b0s0";
        let gs = WorldLightmapState::default();
        let a = render_quad(gpu, off, wr, view_proj, draw, name, 0, 0, gs, false, args)?;
        println!(
            "[lightmapshot] {name}: OBSERVED readback ({}, {}, {}) — CPU lm is NaN (not byte-converted)",
            a[0], a[1], a[2]
        );
        for c in 0..3 {
            if a[c] > 1 {
                failures.push(format!(
                    "{name} channel {c}: NaN store readback {} > 1 (expected ~black)",
                    a[c]
                ));
            }
        }
    }

    // --- Case 6: the darkness terms via `darkness_lightmap`.
    {
        // Every fixture uses the partial-tick 26.2 `GameRenderer.extract` fixes:
        // it calls `calculateDarknessScale(camera, blend, 1.0F)` — the extract
        // happens at the fully-advanced end of the tick, so `partial` is the
        // fixed literal `1.0`, NOT `0.0`. The earlier `0.0` here mis-phased the
        // darkness cosine by one tick.
        const PARTIAL: f32 = 1.0;
        println!(
            "[lightmapshot] darkness partial pinned to {PARTIAL} (26.2 GameRenderer.extract(..,1.0F))"
        );
        // (a) positive darkness: gamma .5, option 1, blend .3, tick 0
        //     (cos((0-1)·π·.025) ≈ +0.997 → positive).
        let (bf_pos, ds_pos) = darkness_lightmap(0.5, 1.0, 0.3, 0, PARTIAL);
        // (b) zero pulse: same but tick 22 (cos negative → darkness clamps 0).
        let (bf_zero, ds_zero) = darkness_lightmap(0.5, 1.0, 0.3, 22, PARTIAL);
        // (c) option .5, blend 1, tick 0: darkness carries the option twice.
        let (_bf_dbl, ds_dbl) = darkness_lightmap(0.5, 0.5, 1.0, 0, PARTIAL);
        println!(
            "[lightmapshot] darkness pos(bright {bf_pos:.4}, dark {ds_pos:.4}) zero(bright {bf_zero:.4}, dark {ds_zero:.4}) dbl-opt(dark {ds_dbl:.4})"
        );

        // Pure-math property asserts (no render needed).
        if ds_zero != 0.0 {
            failures.push(format!(
                "darkness zero-pulse (tick 22) expected 0, got {ds_zero}"
            ));
        }
        // darkness = max(0, cos((tick − partial)·π·0.025) · 0.45 · modifier)
        //            · option, with modifier = blend · option, so the option is
        // applied twice. Reconstruct the expected independently through the same
        // `Mth.cos` table the source uses (NOT assuming cos = 1): at tick 0,
        // partial 1.0 the cosine is ≈0.997, so the naive `0.45·option²` would be
        // wrong. blend 1.0, option 0.5 → modifier 0.5.
        let dbl_modifier = 1.0f32 * 0.5;
        let dbl_cos = mth_cos((0.0 - PARTIAL) * std::f32::consts::PI * 0.025);
        let expect_dbl = (dbl_cos * 0.45 * dbl_modifier).max(0.0) * 0.5;
        println!(
            "[lightmapshot] darkness dbl-opt: cos {dbl_cos:.6} → expected {expect_dbl:.8}, actual {ds_dbl:.8}"
        );
        if (ds_dbl - expect_dbl).abs() > 1e-6 {
            failures.push(format!(
                "darkness double-option multiply: {ds_dbl} != {expect_dbl} (max(0, Mth.cos·0.45·blend·option)·option)"
            ));
        }

        // Render a non-clamped cell (block 7, sky 4 stays positive after the
        // ~0.135 subtraction) for both the positive and the zero darkness.
        let name_pos = "darkness-pos-b7s4";
        let gp = WorldLightmapState {
            brightness_factor: bf_pos,
            darkness_scale: ds_pos,
            ..Default::default()
        };
        let (ep, lmp) = expected_srgb(7, 4, &cpu_state(&gp));
        let ap = render_quad(
            gpu, off, wr, view_proj, draw, name_pos, 7, 4, gp, false, args,
        )?;
        compare_case(failures, name_pos, ap, ep, lmp);

        let name_zero = "darkness-zero-b7s4";
        let gz = WorldLightmapState {
            brightness_factor: bf_zero,
            darkness_scale: ds_zero,
            ..Default::default()
        };
        let (ez, lmz) = expected_srgb(7, 4, &cpu_state(&gz));
        let az = render_quad(
            gpu, off, wr, view_proj, draw, name_zero, 7, 4, gz, false, args,
        )?;
        compare_case(failures, name_zero, az, ez, lmz);

        // The only difference between the two states is the darkness
        // subtraction, so the outputs must differ measurably.
        let d = max_delta(ap, az);
        println!("[lightmapshot] darkness positive vs zero max channel Δ {d}");
        if d < 3 {
            failures.push(format!(
                "darkness positive vs zero differ by only {d} (<3, expected measurable)"
            ));
        }
    }

    // --- Case 7: water. block 4, sky 4, gamma .5, translucent alpha-255 quad.
    // CPU-matches the same lightmap; and because alpha is 255 the water blend
    // collapses to the opaque result, so the equivalent opaque render must
    // match the water render within tolerance.
    {
        let gw = WorldLightmapState {
            brightness_factor: 0.5,
            ..Default::default()
        };
        let name_w = "water-b4s4-g.5";
        let (ew, lmw) = expected_srgb(4, 4, &cpu_state(&gw));
        let aw = render_quad(gpu, off, wr, view_proj, draw, name_w, 4, 4, gw, true, args)?;
        compare_case(failures, name_w, aw, ew, lmw);

        let name_o = "water-opaque-b4s4-g.5";
        let ao = render_quad(gpu, off, wr, view_proj, draw, name_o, 4, 4, gw, false, args)?;
        let d = max_delta(aw, ao);
        println!("[lightmapshot] water (alpha 255) vs opaque max channel Δ {d}");
        if d > CHANNEL_TOL {
            failures.push(format!(
                "water (alpha 255) vs opaque differ by {d} (>{CHANNEL_TOL})"
            ));
        }
    }

    // The terrain/water quad in column 0 is no longer needed and would occlude
    // the capsule, so drop it and let its buffers retire before the entity pass.
    wr.remove_column(gpu, 0, 0);
    gpu.wait_idle();

    // --- Case 8: entity light transport. Entities don't sample the lightmap
    // texture; the caller hands each `EntityDraw` a per-channel `light: [f32;3]`
    // (its eye's lightmap colour) and CPU vertex generation multiplies the
    // model's scalar directional face shade by it. The production entity
    // shader then transports those RGB vertex colors. This case proves that `[f32;3]`
    // survives the CPU→GPU→readback round trip in the production render path.
    run_entity_case(gpu, off, wr, draw, args, failures)?;

    Ok(())
}

/// The entity light-transport case. Renders one capsule lit by the distinctive
/// warm block-light colour (block 7, sky 0, factor 1.0, gamma 0 — the same
/// state as the tint case, non-NaN), reads it back, and proves the readback's
/// normalized linear RGB ratios match the CPU light's. The per-face shade is a
/// scalar, so it cancels under the max-normalization — only the `[f32;3]` light
/// transport is under test, not the lighting geometry.
fn run_entity_case(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    args: &LightmapshotArgs,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let name = "entity-warm-b7s0";

    // The distinctive warm state: block 7, sky 0, block_factor 1.0, gamma 0.
    // `sample` returns finite linear RGB here (block > 0, so no 0/0 NaN); the
    // warm tint makes R > G > B, so the ratio check exercises all three
    // channels rather than a grey.
    let gs = WorldLightmapState {
        block_factor: 1.0,
        ..Default::default()
    };
    let light = sample(7, 0, &cpu_state(&gs));
    if light.iter().any(|c| !c.is_finite()) {
        return Err(format!(
            "{name}: CPU light is non-finite {light:?} — pick a lit state"
        ));
    }

    // No font, no mob textures → every kind (incl. Capsule) falls back to the
    // capsule shell. Cheap, no jar, no server.
    wr.init_entities(gpu, None, MobTextures::default())?;

    // Stable horizontal camera looking down −Z at the capsule, up = +Y.
    let eye = Vec3::new(0.0, 1.0, 3.0);
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let up = Vec3::Y;
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, up);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let view_proj = (proj * view).to_cols_array_2d();
    let right = dir.cross(up).normalize_or_zero().to_array();

    // One capsule at the origin, feet at y=0, lit by the warm light. All pose
    // fields neutral.
    let capsule = EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 1.0,
        height: 2.0,
        color: [1.0, 1.0, 1.0],
        name: None,
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        shell: false,
        skin_uv: None,
        scale_mul: 1.0,
        anim_id: 0.0,
        light,
    };
    wr.set_entities(&[capsule], right, up.to_array(), 0.0);

    off.render(gpu, Some((&mut *wr, view_proj)), draw, CLEAR)?;
    let img = off.read_rgba(gpu)?;
    if let Some(dir) = &args.out_dir {
        let _ = off.save_png(gpu, &dir.join(format!("{name}.png")));
    }

    // Scan every non-black pixel (max RGB byte > 2), decode sRGB → linear, and
    // sum RGB. Directional shade varies per pixel but is scalar, so the summed
    // RGB is `light · Σshade`; normalizing by the max channel cancels `Σshade`.
    let mut sum = [0f64; 3];
    let mut lit_pixels = 0usize;
    for px in img.chunks_exact(4) {
        if *px[..3].iter().max().unwrap() <= 2 {
            continue;
        }
        lit_pixels += 1;
        for c in 0..3 {
            sum[c] += srgb_to_linear(px[c] as f32 / 255.0) as f64;
        }
    }

    // The capsule fills a large fraction of the 128² frame; a measured run
    // yields ≈2.4k lit pixels, so require a robust floor well under that but
    // far above noise.
    const MIN_LIT_PIXELS: usize = 1000;
    if lit_pixels < MIN_LIT_PIXELS {
        failures.push(format!(
            "{name}: only {lit_pixels} lit pixels (< {MIN_LIT_PIXELS}) — camera/mask wrong, cannot grade"
        ));
        return Ok(());
    }

    let act_max = sum[0].max(sum[1]).max(sum[2]);
    let exp_max = light[0].max(light[1]).max(light[2]) as f64;
    let act_norm = [sum[0] / act_max, sum[1] / act_max, sum[2] / act_max];
    let exp_norm = [
        light[0] as f64 / exp_max,
        light[1] as f64 / exp_max,
        light[2] as f64 / exp_max,
    ];
    println!(
        "[lightmapshot] {name}: {lit_pixels} lit px, actual norm ({:.4}, {:.4}, {:.4}) expected norm ({:.4}, {:.4}, {:.4}) [CPU light {:.4},{:.4},{:.4}]",
        act_norm[0], act_norm[1], act_norm[2], exp_norm[0], exp_norm[1], exp_norm[2], light[0], light[1], light[2]
    );
    const RATIO_TOL: f64 = 0.04;
    for c in 0..3 {
        let e = (act_norm[c] - exp_norm[c]).abs();
        if e > RATIO_TOL {
            failures.push(format!(
                "{name} channel {c}: normalized light ratio {:.4} vs expected {:.4} (|Δ| {e:.4} > {RATIO_TOL})",
                act_norm[c], exp_norm[c]
            ));
        }
    }

    Ok(())
}

/// Upload the shared white quad — as opaque terrain (`translucent = false`) or
/// translucent water (`translucent = true`) — under `gpu_state`, render the
/// production world path, and return the sampled centre-patch sRGB bytes.
///
/// The texel and vertex colour are both white, so the fragment's
/// `c.rgb * v_color * lm` stores exactly the lightmap; the water pass uses the
/// alpha-255 texel (its blend then collapses to the opaque result).
#[allow(clippy::too_many_arguments)]
fn render_quad(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    view_proj: [[f32; 4]; 4],
    draw: &OverlayDraw,
    name: &str,
    block: u8,
    sky: u8,
    gpu_state: WorldLightmapState,
    translucent: bool,
    args: &LightmapshotArgs,
) -> Result<[u8; 3], String> {
    wr.set_lightmap_state(gpu_state);

    // One large top-facing white quad inside column 0 (x,z ∈ [0,16]) at y=64,
    // packed with the case's block/sky levels. Winding is irrelevant (the
    // world pipeline culls no faces).
    let layer_word = pack_layer(0, block, sky);
    let color = [1.0f32, 1.0, 1.0];
    let y = 64.0f32;
    let verts = [
        MeshVertex {
            pos: [0.0, y, 0.0],
            uv: [0.0, 0.0],
            layer: layer_word,
            color,
        },
        MeshVertex {
            pos: [16.0, y, 0.0],
            uv: [1.0, 0.0],
            layer: layer_word,
            color,
        },
        MeshVertex {
            pos: [16.0, y, 16.0],
            uv: [1.0, 1.0],
            layer: layer_word,
            color,
        },
        MeshVertex {
            pos: [0.0, y, 16.0],
            uv: [0.0, 1.0],
            layer: layer_word,
            color,
        },
    ];
    let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];
    let vertex_bytes: &[u8] = bytemuck::cast_slice(&verts);

    // Give the AABB a little thickness so the GPU cull's box test (opaque) and
    // the translucent visibility test never reject a zero-height slab. The quad
    // goes into the opaque OR the translucent region depending on the pass.
    if translucent {
        wr.upload_column(
            gpu,
            0,
            0,
            &[],
            &[],
            vertex_bytes,
            &indices,
            y - 1.0,
            y + 1.0,
        )?;
    } else {
        wr.upload_column(
            gpu,
            0,
            0,
            vertex_bytes,
            &indices,
            &[],
            &[],
            y - 1.0,
            y + 1.0,
        )?;
    }
    gpu.wait_idle();

    off.render(gpu, Some((&mut *wr, view_proj)), draw, CLEAR)?;
    let img = off.read_rgba(gpu)?;
    if let Some(dir) = &args.out_dir {
        let _ = off.save_png(gpu, &dir.join(format!("{name}.png")));
    }
    Ok(center_avg(&img, W, H, 8))
}

/// Mirror a GPU `WorldLightmapState` into the CPU `LightmapState` so the two
/// oracles are driven by byte-identical inputs.
fn cpu_state(g: &WorldLightmapState) -> LightmapState {
    LightmapState {
        sky_factor: g.sky_factor,
        block_factor: g.block_factor,
        sky_light_color: g.sky_color,
        brightness_factor: g.brightness_factor,
        darkness_scale: g.darkness_scale,
        night_vision_factor: g.night_vision_factor,
    }
}

/// The independent CPU expectation: sample the lightmap, encode linear→sRGB,
/// round to bytes. Returns `(sRGB bytes, linear lm)`. The caller must not use
/// this for the black-texel NaN case (`sample` returns NaN there).
fn expected_srgb(block: u8, sky: u8, cpu: &LightmapState) -> ([u8; 3], [f32; 3]) {
    let lm = sample(block, sky, cpu);
    let px: [u8; 3] =
        std::array::from_fn(|c| (linear_to_srgb(lm[c]) * 255.0).round().clamp(0.0, 255.0) as u8);
    (px, lm)
}

/// Log a case's actual-vs-expected and accumulate any per-channel failures.
fn compare_case(
    failures: &mut Vec<String>,
    name: &str,
    actual: [u8; 3],
    expected: [u8; 3],
    lm: [f32; 3],
) {
    println!(
        "[lightmapshot] {name}: actual ({}, {}, {}) expected ({}, {}, {}) [linear lm {:.4},{:.4},{:.4}]",
        actual[0], actual[1], actual[2], expected[0], expected[1], expected[2], lm[0], lm[1], lm[2]
    );
    for c in 0..3 {
        let diff = (actual[c] as i32 - expected[c] as i32).abs();
        if diff > CHANNEL_TOL {
            failures.push(format!(
                "{name} channel {c}: actual {} vs expected {} (|Δ| {diff} > {CHANNEL_TOL})",
                actual[c], expected[c]
            ));
        }
    }
}

/// Rec.709 luma of an sRGB byte triple (relative ordering only).
fn luma(px: [u8; 3]) -> f32 {
    0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32
}

/// Max absolute per-channel byte difference between two samples.
fn max_delta(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|c| (a[c] as i32 - b[c] as i32).abs())
        .max()
        .unwrap_or(0)
}

/// Park the frame-time overlay off-screen so it never covers the sampled patch.
fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

/// Average RGB over a centred square of half-size `r`.
fn center_avg(img: &[u8], w: u32, h: u32, r: u32) -> [u8; 3] {
    let (cx, cy) = (w / 2, h / 2);
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for py in cy.saturating_sub(r)..(cy + r).min(h) {
        for px in cx.saturating_sub(r)..(cx + r).min(w) {
            let i = ((py * w + px) * 4) as usize;
            for c in 0..3 {
                sum[c] += img[i + c] as f64;
            }
            n += 1.0;
        }
    }
    std::array::from_fn(|c| (sum[c] / n).round() as u8)
}

/// Linear → sRGB encode — the transfer function the `R8G8B8A8_SRGB` attachment
/// applies on store (standard piecewise IEC 61966-2-1 formula).
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → linear decode — the exact inverse of [`linear_to_srgb`], used to turn
/// the entity readback's sRGB bytes back into the linear space the CPU light
/// lives in before comparing channel ratios.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
