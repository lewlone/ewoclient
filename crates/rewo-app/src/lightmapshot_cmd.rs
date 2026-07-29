//! `rewo lightmapshot` — serverless headless verification of the production
//! world lightmap (M13).
//!
//! The M12 skyshot gate verifies the *sky*; this gate verifies the *lightmap*
//! the terrain fragment shader (`shaders/lightmap.glsl`, included by both
//! `world.frag` and `water.frag`) computes from a vertex's packed
//! `(block, sky)` levels.
//!
//! This is a **TERRAIN + WATER + COLOR + ENTITY** matrix. The terrain/water
//! cases render the same large white quad — as opaque terrain geometry or as
//! translucent water geometry — through the real `WorldRenderer`, then compare
//! the GPU readback against an independent CPU lightmap. It is a
//! GPU-vs-independent-CPU oracle, never a "looks right" proxy. The color case
//! covers the one thing an all-white matrix structurally cannot: M15 stopped
//! storing the per-vertex colour and now reconstructs it in `world.vert` from
//! the packed shade/AO codes plus tint bytes, so that product exists only on
//! the GPU — and an exact-white quad reconstructs to `1.0 * 1.0 * (255/255)`,
//! the single input under which a broken reconstruction still reads correct.
//!
//! The final case proves the **entity** light transport: entities don't sample the lightmap
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
//!   blend collapses to the opaque result (the write mask is RGB only). The
//!   color case is the deliberate exception: it drives the same quad with
//!   non-white shade/AO/tint, so `c.rgb * v_color * lm` stays `color * lm`.
//! - Sample an interior centre patch → the actual sRGB bytes.
//! - Independently compute the expected lightmap from
//!   `rewo_world::lightmap::sample(block, sky, ..)` with the *matching* state,
//!   encode linear→sRGB with the standard piecewise formula, round to bytes.
//! - Require each channel within ±2, and assert the case-specific property
//!   (warm tint, block-factor response, gamma ramp, night-vision lift, the
//!   black-texel NaN store, the darkness terms, water==opaque, the packed
//!   vertex-colour reconstruction, entity `[f32; 3]` light transport).

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_gpu::entities::{EntityDraw, EntityModelKind, MobTextures};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldLightmapState, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::{pack_layer, MeshVertex, TINT_WHITE};
use rewo_world::lightmap::{
    darkness_lightmap, mth_cos, rgb24_to_vec3, sample, LightmapState, DEFAULT_AMBIENT_COLOR,
};

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
            println!("[lightmapshot] CHECK OK — terrain + water + packed color + dimension ambient + entity lightmap matrix verified");
        } else {
            println!("[lightmapshot] PASS — terrain + water + packed color + dimension ambient + entity lightmap matrix verified (assertion mode off)");
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

    // --- Case 8: the M15 packed vertex colour. Every case above renders an
    // exact-white quad so it isolates the lightmap — which also means none of
    // them exercises the reconstruction `world.vert` now performs. M15 stopped
    // storing the per-vertex colour and rebuilt it on the GPU from the discrete
    // shade/AO codes plus lossless tint bytes; white is precisely the input
    // (`1.0 * 1.0 * 255/255`) under which a broken reconstruction still reads
    // correct, so only a non-white render can grade it.
    //
    // The inputs are chosen non-degenerate in every factor: shade 1 (0.5, not
    // the 1.0 identity), AO 1 (0.65, not the AO_NONE identity), and a tint whose
    // three bytes differ and none of which is 255. A dropped shade bit, a
    // swapped AO index, a channel transpose, or a regression in the `/255`
    // divide `world.vert`'s `precise` qualifier protects all move the result.
    {
        const SHADE: u8 = 1; // FACE_SHADE[1] == 0.5
        const AO: u8 = 1; // AO_LEVELS[1] == 0.65
        const TINT: [u8; 3] = [120, 200, 60];
        // Mid-lit and non-clamped, so the colour factors show up as real
        // dimming rather than saturating against the top of the range.
        const BLOCK: u8 = 12;
        const SKY: u8 = 8;
        let gs = WorldLightmapState {
            block_factor: 1.0,
            brightness_factor: 0.5,
            ..Default::default()
        };

        // Pin the packing round-trip on the CPU *before* grading pixels, so a
        // bit-layout regression fails as itself rather than as a colour
        // mismatch. Release asserts — lightmapshot runs in release, where a
        // `debug_assert!` would silently compile out.
        let probe = MeshVertex::new([0.0, 64.0, 0.0], [0.0, 0.0], 0, BLOCK, SKY, SHADE, AO, TINT);
        assert_eq!(probe.shade_code(), SHADE, "shade code must survive packing");
        assert_eq!(probe.ao_code(), AO, "AO code must survive packing");
        assert_eq!(probe.tint_rgb(), TINT, "tint bytes must survive packing");
        assert_eq!(
            probe.light & 0x00FF_FFFF,
            pack_layer(0, BLOCK, SKY),
            "the shade/AO bits must not disturb the historical layer/light word"
        );
        let scalar = rewo_mesh::FACE_SHADE[SHADE as usize] * rewo_mesh::AO_LEVELS[AO as usize];
        let color: [f32; 3] = std::array::from_fn(|c| scalar * (TINT[c] as f32 / 255.0));
        println!(
            "[lightmapshot] packed-colour inputs: shade {SHADE} (×{:.2}) · AO {AO} (×{:.2}) · tint {TINT:?} → colour ({:.4}, {:.4}, {:.4})",
            rewo_mesh::FACE_SHADE[SHADE as usize],
            rewo_mesh::AO_LEVELS[AO as usize],
            color[0],
            color[1],
            color[2]
        );

        // (a) Absolute: the texel is white, so the stored linear value is
        //     `colour * lm` — graded against the independent CPU product.
        let name_c = "packed-color-b12s8";
        let (ec, lmc) = expected_srgb_colored(BLOCK, SKY, &cpu_state(&gs), color);
        let ac = render_quad_colored(
            gpu, off, wr, view_proj, draw, name_c, BLOCK, SKY, gs, false, args, SHADE, AO, TINT,
        )?;
        compare_case(failures, name_c, ac, ec, lmc);

        // (b) Ratio, independent of the CPU lightmap: render the SAME cell as
        //     the exact-white quad and take the per-channel linear ratio. The
        //     lightmap is identical in both renders, so it divides out and what
        //     remains is purely the reconstructed colour — this check still
        //     holds even if `sample` itself were wrong, which the absolute
        //     comparison above cannot say.
        let name_w = "packed-color-white-b12s8";
        let aw = render_quad(
            gpu, off, wr, view_proj, draw, name_w, BLOCK, SKY, gs, false, args,
        )?;
        const RATIO_TOL: f32 = 0.03;
        // Below this the white reference is too dark for the sRGB byte
        // quantization to carry a meaningful ratio; report rather than grade.
        const RATIO_FLOOR: f32 = 0.05;
        for c in 0..3 {
            let white_lin = srgb_to_linear(aw[c] as f32 / 255.0);
            let colored_lin = srgb_to_linear(ac[c] as f32 / 255.0);
            if white_lin < RATIO_FLOOR {
                failures.push(format!(
                    "packed-color channel {c}: white reference linear {white_lin:.4} < {RATIO_FLOOR} — too dark to grade the ratio"
                ));
                continue;
            }
            let ratio = colored_lin / white_lin;
            let e = (ratio - color[c]).abs();
            println!(
                "[lightmapshot] packed-color channel {c}: colored/white linear ratio {ratio:.4} vs reconstructed {:.4} (|Δ| {e:.4})",
                color[c]
            );
            if e > RATIO_TOL {
                failures.push(format!(
                    "packed-color channel {c}: linear ratio {ratio:.4} vs reconstructed colour {:.4} (|Δ| {e:.4} > {RATIO_TOL})",
                    color[c]
                ));
            }
        }
    }

    // --- Case 10 (M16): the per-dimension `AmbientColor`.
    //
    // `LightmapRenderStateExtractor.extract` reads `AmbientColor` from the
    // camera's attribute probe (`EnvironmentAttributes.AMBIENT_LIGHT_COLOR`),
    // so it is a per-dimension value, not the shader constant the pre-M16 code
    // baked in. It reaches the GPU through the world pass's small
    // `LightmapExtra` uniform buffer, because `WorldPush` is exactly at
    // Vulkan's guaranteed 128-byte push budget.
    //
    // The colours are the built-in dimension types' literal attribute values
    // (`data/minecraft/dimension_type/*.json`), converted by
    // `ARGB.vector3fFromRGB24` (a plain /255, no sRGB decode).
    {
        // Overworld #0a0a0a, Nether #302821, End #3f473f.
        const OW: i32 = 0xFF0A_0A0Au32 as i32;
        const NETHER: i32 = 0xFF30_2821u32 as i32;
        const END: i32 = 0xFF3F_473Fu32 as i32;
        let amb = |argb: i32| rgb24_to_vec3(argb);
        println!(
            "[lightmapshot] ambient sources: OW {:?} Nether {:?} End {:?} (default {:?})",
            amb(OW),
            amb(NETHER),
            amb(END),
            DEFAULT_AMBIENT_COLOR
        );
        // Release assert: the attribute default really is black, so every case
        // above (which leaves `ambient_color` at its default) is unaffected.
        assert_eq!(
            WorldLightmapState::default().ambient_color,
            DEFAULT_AMBIENT_COLOR,
            "the GPU state default must stay the attribute default"
        );
        assert_eq!(DEFAULT_AMBIENT_COLOR, [0.0, 0.0, 0.0]);

        // (a) The unlit texel. block 0, sky 0, no night vision, gamma 0: the
        //     whole lightmap IS the ambient (`max(ambient, nv*0)`, nothing
        //     added, notGamma mixed in at weight 0). Graded against the CPU.
        let mut unlit: Vec<(&str, [u8; 3])> = Vec::new();
        for (tag, argb) in [("ow", OW), ("nether", NETHER), ("end", END)] {
            let name = format!("ambient-{tag}-b0s0");
            let gs = WorldLightmapState {
                ambient_color: amb(argb),
                ..Default::default()
            };
            let (e, lm) = expected_srgb(0, 0, &cpu_state(&gs));
            let a = render_quad(gpu, off, wr, view_proj, draw, &name, 0, 0, gs, false, args)?;
            compare_case(failures, &name, a, e, lm);
            unlit.push((tag, a));
        }

        // (b) The mutation/sensitivity check: the SAME unlit cell with the
        //     default (black) ambient stores ~black (the documented 0/0 NaN
        //     path). So the ambient term genuinely owns these pixels — a shader
        //     that ignored the new uniform would render all four identically.
        let black = render_quad(
            gpu,
            off,
            wr,
            view_proj,
            draw,
            "ambient-default-b0s0",
            0,
            0,
            WorldLightmapState::default(),
            false,
            args,
        )?;
        println!(
            "[lightmapshot] ambient-default-b0s0 readback ({}, {}, {}) — the black-ambient baseline",
            black[0], black[1], black[2]
        );
        for (tag, a) in &unlit {
            let d = max_delta(*a, black);
            println!("[lightmapshot] ambient {tag} vs black-ambient max channel Δ {d}");
            // Even the Overworld's dim #0a0a0a is 10/255 linear ≈ sRGB 89.
            if d < 20 {
                failures.push(format!(
                    "ambient {tag}: only Δ{d} vs the black-ambient baseline (<20) — the ambient uniform does not own the pixel"
                ));
            }
        }
        // And the three dimensions must be distinguishable from each other, so
        // the check cannot pass on a single hard-coded non-black constant.
        for (i, (ta, a)) in unlit.iter().enumerate() {
            for (tb, b) in unlit.iter().skip(i + 1) {
                let d = max_delta(*a, *b);
                if d < 10 {
                    failures.push(format!(
                        "ambient {ta} vs {tb}: max channel Δ{d} (<10) — dimensions not distinguishable"
                    ));
                }
            }
        }
        // The End's ambient is greenish (#3f473f: G > R == B); the readback must
        // carry that ordering, which a channel transpose in the UBO would break.
        let end_px = unlit[2].1;
        if !(end_px[1] > end_px[0] && end_px[0] == end_px[2]) {
            failures.push(format!(
                "ambient end: readback {end_px:?} does not carry #3f473f's G>R==B ordering"
            ));
        }

        // (c) A mid-lit, non-clamped cell — ambient must also shift a pixel
        //     that already has real sky and block light, not just the black
        //     texel. Both CPU-graded, and measurably apart from each other.
        let mid_amb = WorldLightmapState {
            ambient_color: amb(NETHER),
            ..Default::default()
        };
        let (em, lmm) = expected_srgb(4, 4, &cpu_state(&mid_amb));
        let am = render_quad(
            gpu,
            off,
            wr,
            view_proj,
            draw,
            "ambient-nether-b4s4",
            4,
            4,
            mid_amb,
            false,
            args,
        )?;
        compare_case(failures, "ambient-nether-b4s4", am, em, lmm);
        let ap = render_quad(
            gpu,
            off,
            wr,
            view_proj,
            draw,
            "ambient-default-b4s4",
            4,
            4,
            WorldLightmapState::default(),
            false,
            args,
        )?;
        let dm = max_delta(am, ap);
        println!("[lightmapshot] ambient mid-lit (b4s4) nether vs default max channel Δ {dm}");
        if dm < 10 {
            failures.push(format!(
                "ambient mid-lit: nether vs default differ by only {dm} (<10)"
            ));
        }

        // (d) The translucent pass reads the SAME uniform buffer. Opaque and
        //     water geometry must resolve an identical lightmap — the alpha-255
        //     texel collapses the blend to the opaque result.
        let water_amb = WorldLightmapState {
            ambient_color: amb(END),
            ..Default::default()
        };
        let (ew, lmw) = expected_srgb(4, 4, &cpu_state(&water_amb));
        let aw = render_quad(
            gpu,
            off,
            wr,
            view_proj,
            draw,
            "ambient-end-water-b4s4",
            4,
            4,
            water_amb,
            true,
            args,
        )?;
        compare_case(failures, "ambient-end-water-b4s4", aw, ew, lmw);
        let ao = render_quad(
            gpu,
            off,
            wr,
            view_proj,
            draw,
            "ambient-end-opaque-b4s4",
            4,
            4,
            water_amb,
            false,
            args,
        )?;
        let dw = max_delta(aw, ao);
        println!("[lightmapshot] ambient water vs opaque max channel Δ {dw}");
        if dw > CHANNEL_TOL {
            failures.push(format!(
                "ambient water vs opaque differ by {dw} (>{CHANNEL_TOL}) — water.frag missed the ambient uniform"
            ));
        }
    }


    // --- Case 9 (M33b): the two fog bands, and that the shader takes their MAX.
    //
    // Rewo's world pass carries two independent fog bands. The push block's is
    // a render-distance fade whose job is dissolving the chunk edge into the
    // sky; the `LightmapExtra` UBO's is the **environmental** band, which is
    // the one rain thickens (`rainFogMultiplier`). Vanilla's `total_fog_value`
    // is the `max` of an environmental and a render-distance term, and this
    // pins that it really is a max — not a sum, a product, a min, or a
    // replacement, each of which would land somewhere else here.
    //
    // The camera sits at (8, 80, 8) looking straight down at the quad's plane
    // at y = 64, so every sampled pixel is 16.0 blocks away and the fog
    // fraction is exact rather than approximated.
    {
        const DIST: f32 = 16.0;
        // A saturated fog colour, so a channel swap or a wrong blend direction
        // cannot hide inside a grey.
        const FOG_LINEAR: [f32; 3] = [0.8, 0.1, 0.0];
        // Full sky light and no block light: a bright, flat quad to fog.
        let state = |env: [f32; 2]| WorldLightmapState {
            env_fog: env,
            ..Default::default()
        };
        let disabled = [1.0e9f32, 1.0e9 + 1.0];
        let frac = |start: f32, end: f32| ((DIST - start) / (end - start)).clamp(0.0, 1.0);

        wr.set_sky_tint([0.0; 3], [1.0, 1.0, 1.0]);
        wr.set_sky_fog_base([0.0; 3], FOG_LINEAR);

        // The unfogged baseline: BOTH bands disabled.
        wr.set_fog(1.0e6, 1.0e6 + 1.0);
        let clear = render_quad(
            gpu, off, wr, view_proj, draw, "fog-none", 0, 15, state(disabled), false, args,
        )?;

        // What the shader must produce for a given fog fraction. The mix runs
        // in LINEAR space inside the shader and the attachment re-encodes on
        // store, so the prediction has to make the same round trip.
        let predict = |f: f32| -> [u8; 3] {
            let mut out = [0u8; 3];
            for c in 0..3 {
                let base = srgb_to_linear(clear[c] as f32 / 255.0);
                let mixed = base + (FOG_LINEAR[c] - base) * f;
                out[c] = (linear_to_srgb(mixed.clamp(0.0, 1.0)) * 255.0).round() as u8;
            }
            out
        };
        let close = |a: [u8; 3], b: [u8; 3]| {
            (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= 2)
        };

        // (a) The environmental band alone, with the render-distance band off.
        let env_only = render_quad(
            gpu, off, wr, view_proj, draw, "fog-env", 0, 15, state([0.0, 32.0]), false, args,
        )?;
        let want_half = predict(frac(0.0, 32.0));
        println!(
            "[lightmapshot] fog-env: dist {DIST} band (0, 32) -> f {:.3}; \
             read {env_only:?} predicted {want_half:?} (unfogged {clear:?})",
            frac(0.0, 32.0)
        );
        if !close(env_only, want_half) {
            failures.push(format!(
                "fog-env: read {env_only:?}, predicted {want_half:?} from the unfogged \
                 {clear:?} — the environmental band is not reaching the shader, or its \
                 blend is not `mix(color, fog, f)` in linear space"
            ));
        }
        // It must actually have done something, or the case grades nothing.
        if close(env_only, clear) {
            failures.push(format!(
                "fog-env: {env_only:?} is indistinguishable from the unfogged \
                 {clear:?} — the band had no effect at all"
            ));
        }

        // (b) The render-distance band alone, same numbers. The two bands are
        // symmetric inputs to the max, so this must land on the same colour.
        wr.set_fog(0.0, 32.0);
        let rd_only = render_quad(
            gpu, off, wr, view_proj, draw, "fog-rd", 0, 15, state(disabled), false, args,
        )?;
        if !close(rd_only, env_only) {
            failures.push(format!(
                "fog-rd: the render-distance band at (0, 32) gives {rd_only:?} where the \
                 environmental band at the same numbers gives {env_only:?} — they feed \
                 one `max` and must be interchangeable"
            ));
        }

        // (c) The max itself. Render-distance (0, 32) gives 0.5; environmental
        // (0, 64) gives 0.25. Only `max` lands on 0.5:
        //   max -> 0.5   sum -> 0.75   product -> 0.125   min -> 0.25
        // and each of those is far outside the tolerance.
        let both = render_quad(
            gpu, off, wr, view_proj, draw, "fog-max", 0, 15, state([0.0, 64.0]), false, args,
        )?;
        let want_max = predict(0.5f32.max(0.25));
        let want_sum = predict(0.75);
        let want_min = predict(0.25);
        println!(
            "[lightmapshot] fog-max: rd 0.5 + env 0.25 -> read {both:?}; max {want_max:?} \
             sum {want_sum:?} min {want_min:?}"
        );
        if !close(both, want_max) {
            failures.push(format!(
                "fog-max: read {both:?}, expected the MAX {want_max:?} (a sum would give \
                 {want_sum:?}, a min {want_min:?})"
            ));
        }
        if close(want_max, want_min) || close(want_max, want_sum) {
            failures.push(format!(
                "fog-max is vacuous: max {want_max:?}, sum {want_sum:?} and min \
                 {want_min:?} are not far enough apart to tell them apart"
            ));
        }

        // (d) The weaker band must not pull the stronger one down — the whole
        // point of a max. Environmental (0, 1024) at 16 blocks is ~0.016, so
        // this must stay on the render-distance band's 0.5.
        let weak = render_quad(
            gpu, off, wr, view_proj, draw, "fog-weak-env", 0, 15, state([0.0, 1024.0]), false, args,
        )?;
        if !close(weak, want_max) {
            failures.push(format!(
                "fog-weak-env: a nearly-disabled environmental band changed the result to \
                 {weak:?} from {want_max:?} — a max cannot be reduced by its smaller term"
            ));
        }

        // Restore the state the earlier cases assume, so ordering stays free.
        wr.set_fog(1.0e6, 1.0e6 + 1.0);
        wr.set_sky_tint([0.0; 3], [0.0; 3]);
        wr.set_sky_fog_base([0.0; 3], [0.0; 3]);
    }

    // The terrain/water quad in column 0 is no longer needed and would occlude
    // the capsule, so drop it and let its buffers retire before the entity pass.
    wr.remove_column(gpu, 0, 0);
    gpu.wait_idle();

    // --- Case 10: entity light transport. Entities don't sample the lightmap
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
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
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
    // Shade code 0 (`FACE_SHADE[0] == 1.0`) + AO_NONE (`AO_LEVELS[3] == 1.0`) +
    // white tint bytes reconstruct to EXACTLY (1,1,1) in the vertex shader:
    // `1.0 * 1.0 * (255/255)`. This quad must stay pure white so it isolates the
    // lightmap; the assertions below pin that on the CPU side too. They are
    // `assert_eq!`, not `debug_assert_eq!` — lightmapshot runs in release, where
    // a debug assertion would silently compile out.
    let probe = MeshVertex::new(
        [0.0, 64.0, 0.0],
        [0.0, 0.0],
        0,
        block,
        sky,
        0,
        rewo_mesh::AO_NONE,
        TINT_WHITE,
    );
    assert_eq!(
        probe.reconstructed_color(),
        [1.0, 1.0, 1.0],
        "the lightmap probe quad must decode to exact white"
    );
    assert_eq!(
        probe.light & 0x00FF_FFFF,
        pack_layer(0, block, sky),
        "packed light word must preserve the pack_layer lower 24 bits"
    );

    render_quad_colored(
        gpu,
        off,
        wr,
        view_proj,
        draw,
        name,
        block,
        sky,
        gpu_state,
        translucent,
        args,
        0,
        rewo_mesh::AO_NONE,
        TINT_WHITE,
    )
}

/// The general form of [`render_quad`]: the same upload → render → readback,
/// with the quad's per-vertex colour left to the caller via `shade_code`,
/// `ao_code` and `tint_rgb`. Passing `(0, AO_NONE, TINT_WHITE)` reproduces the
/// exact-white probe quad.
#[allow(clippy::too_many_arguments)]
fn render_quad_colored(
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
    shade_code: u8,
    ao_code: u8,
    tint_rgb: [u8; 3],
) -> Result<[u8; 3], String> {
    wr.set_lightmap_state(gpu_state);

    // One large top-facing quad inside column 0 (x,z ∈ [0,16]) at y=64, packed
    // with the case's block/sky levels. Winding is irrelevant (the world
    // pipeline culls no faces).
    let y = 64.0f32;
    let vert = |pos: [f32; 3], uv: [f32; 2]| {
        MeshVertex::new(pos, uv, 0, block, sky, shade_code, ao_code, tint_rgb)
    };
    let verts = [
        vert([0.0, y, 0.0], [0.0, 0.0]),
        vert([16.0, y, 0.0], [1.0, 0.0]),
        vert([16.0, y, 16.0], [1.0, 1.0]),
        vert([0.0, y, 16.0], [0.0, 1.0]),
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
        ambient_color: g.ambient_color,
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

/// [`expected_srgb`] with a non-white per-vertex colour folded in.
///
/// The fragment computes `c.rgb * v_color * lm` and the synthetic texel is
/// white, so the stored linear value is `colour * lm`. `expected_srgb` is
/// exactly this specialized to a white colour; passing `[1.0; 3]` here
/// reproduces it. `colour` is derived independently from the legacy
/// `FACE_SHADE`/`AO_LEVELS` tables and the raw tint bytes — not from
/// `MeshVertex`'s own accessor — which is what makes this an independent
/// oracle for the packed-colour case rather than a restatement of the GPU's
/// own arithmetic.
fn expected_srgb_colored(
    block: u8,
    sky: u8,
    cpu: &LightmapState,
    color: [f32; 3],
) -> ([u8; 3], [f32; 3]) {
    let lm = sample(block, sky, cpu);
    let px: [u8; 3] = std::array::from_fn(|c| {
        (linear_to_srgb(lm[c] * color[c]) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    });
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
