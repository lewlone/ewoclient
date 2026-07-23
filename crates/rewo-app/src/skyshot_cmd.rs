//! `rewo skyshot` — serverless headless verification of the clear-weather
//! Overworld sky (M12: sun, moon, stars, sunrise; and the M11 zenith-tint fix).
//!
//! `--check` renders the sky offscreen (no server, no world columns) at chosen
//! world-clock ticks and camera orientations and asserts *properties* of the
//! read-back pixels, never a "looks right" proxy:
//!
//! - **Zenith tint (Part A).** With no celestial pass, the sky disc at midnight
//!   must be ~black overhead and at noon must be blue. The M11 bug tinted only
//!   the horizon, leaving a blue midnight zenith — this rejects it.
//! - **Sun.** At noon the sun sits at the zenith; looking up, the centre must
//!   carry bright *warm* pixels the plain sky lacks.
//! - **Moon.** At midnight the moon sits at the zenith over a ~black sky;
//!   looking up, the centre must be clearly brighter than that black.
//! - **Stars.** Toggling the star-brightness state at midnight must add many
//!   bright points across the sky.
//! - **Sunrise (M12 placement oracle).** A fresh renderer over a black sky with
//!   the bodies hidden and stars off draws the fan with a controlled POSITIVE
//!   `sun_angle` (so vanilla selects fan angle 0 → the bright centre lands on
//!   the −X horizon) and a known warm colour + alpha. The 18 logical Mth-table
//!   fan vertices are reconstructed *here* (never via a `CelestialPass` helper),
//!   pushed through the decompiled `X(90°)·Z(90°)·scale(z=alpha)` pose, and
//!   projected through a horizontal −X camera; the read-back warm ABOVE-HORIZON
//!   footprint (the fan opens upward from its horizon centre) must match that
//!   analytic footprint — vertical band + horizontal centroid — to a small
//!   raster tolerance. The opposite +X heading is a placement guard: it sees
//!   only the fan's faint below-horizon underside, so it must show NO comparable
//!   above-horizon warm fan. This rejects a wrong X/Z order, a flipped sign, a
//!   dropped `scale(z=alpha)`, or the fan placed on the opposite horizon.
//!
//! It also reports the **accepted** star count (`buildStars` rejects some of its
//! 1500 candidates), per the milestone brief.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths};
use rewo_gpu::celestial::{CelestialImage, CelestialState, CelestialTextures};
use rewo_gpu::entities::srgb_to_linear;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_world::{celestial, daylight};

use crate::stats::OverlayRing;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const NOON: i64 = 6000;
const MIDNIGHT: i64 = 18000;

// -- M12: analytic projected-body transform + size oracle ---------------------
/// The nontrivial body angle for the M12 transform check (degrees). +15° is off
/// the zenith and off any half/whole turn, so a flipped sign is unambiguous.
const M12_BODY_ANGLE_DEG: f64 = 15.0;
/// Sun/moon `scale(size,1,size)` factors from `renderSunMoonAndStars` — the
/// size oracle. Swapping them shifts each envelope > 17 px.
const M12_SUN_SIZE: f64 = 30.0;
const M12_MOON_SIZE: f64 = 20.0;
/// Body mask: the isolated bodies are opaque bright solids (channels ≥ 92) over
/// a pure-black sky (0), so any max-channel ≥ 40 is body, not clear/sky.
const M12_BODY_MASK: u8 = 40;
/// Rasterisation tolerance (px): pixel-centre sampling at the projected quad's
/// (vertical) left/right edges leaves ≤ ~0.5 px residual; 2.5 px is small yet
/// far below the ≥ 17 px a swapped 30/20 (or ≥ 98 px a flipped sign) would move
/// the envelope.
const M12_RASTER_TOL: f64 = 2.5;
/// Consistency epsilon for the pure-f64 analytic values vs the senior
/// double-precision cross-check (no rasterisation → exact to < 0.01 px).
const SENIOR_EPS: f64 = 0.05;
/// Senior double-precision cross-check for +15°, the up-camera: the shared
/// projected model-centre and the sun/moon envelopes `(min_x,min_y,max_x,max_y)`.
const SENIOR_CENTER: [f64; 2] = [176.982, 128.000];
const SENIOR_SUN_BBOX: [f64; 4] = [122.577, 66.262, 240.898, 189.738];
const SENIOR_MOON_BBOX: [f64; 4] = [139.790, 88.006, 218.386, 167.994];

// -- M12: analytic sunrise-fan placement oracle -------------------------------
/// Controlled sun angle for the fan-side selection: `renderSunriseAndSunset`
/// picks fan angle 0 (bright centre on the −X horizon) whenever `sin(sun_angle)
/// ≥ 0`. π/2 is unambiguously positive, and with the bodies hidden the sun
/// angle affects nothing else.
const M12_SUNRISE_SUN_ANGLE: f64 = std::f64::consts::FRAC_PI_2;
/// Controlled fan alpha — used BOTH as the `scale(z)` factor AND the tint alpha.
/// Chosen distinctly non-1 so a dropped `scale(z=alpha)` roughly *doubles* the
/// fan's vertical extent (the ring line moves ≈30 px) and fails loudly. Values
/// near vanilla dusk (~0.99) would make that bug invisible.
const M12_SUNRISE_ALPHA: f64 = 0.5;
/// Controlled warm fan colour (LINEAR rgb). r ≫ b so every fan fragment reads
/// warm (red > blue) even at the faded alpha-0 ring edge.
const M12_SUNRISE_RGB: [f32; 3] = [0.9, 0.35, 0.10];
/// Raster tolerance (px) for the fan footprint's vertical band + centroid. The
/// smallest error this must catch moves the footprint ≥ 30 px (dropped alpha
/// scale) or removes it from −X entirely (wrong X/Z order, flipped sign,
/// opposite horizon); 4 px is small yet clears the ring-edge fade + centre
/// taper's sub-pixel raster residue.
const M12_SUNRISE_TOL: f64 = 4.0;
/// Minimum warm above-horizon pixel count for the −X fan to be considered
/// present (a stray speck must not pass). The band is ~30 px tall × most of the
/// 256 px width, so several thousand is typical.
const M12_SUNRISE_MIN_PX: usize = 800;
/// `Mth.SIN_SCALE` (`65536 / 2π`), the sine-table index scale, reconstructed
/// here so the fan geometry is derived independently of `rewo-gpu`.
const MTH_SIN_SCALE: f64 = 10430.378350470453;

#[derive(ClapArgs)]
pub struct SkyshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Assert the sky properties and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Optional: also dump the rendered frames here (eyeball artifact).
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Bypass Vulkan validation layers. Otherwise the gate requests them in
    /// every build and, under `--check`, fails if they don't activate.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: SkyshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    // The permanent gate runs with Vulkan validation ON in every build (debug
    // AND release) — `cfg!(debug_assertions)` would silently drop it from the
    // release binary the gate actually ships as. `--no-validation` is the sole,
    // explicit opt-out.
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;

    // Report the resolved validation status explicitly so a run never claims
    // "validation-on" without it being true. `Gpu::validation_active` is the
    // ground truth (requested AND the layer is installed).
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[skyshot] Vulkan validation: {status}");

    // Under `--check`, requesting validation but not getting it must fail loudly
    // rather than grade a frame with validation silently off. The `--no-validation`
    // escape hatch never trips this (it doesn't request validation).
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "skyshot check: Vulkan validation requested but not active — \
                    install the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass \
                    --no-validation to bypass"
                .into(),
        );
    }

    run_check(&mut gpu, &baked, &args)
}

fn run_check(gpu: &mut Gpu, baked: &assets::BakedAssets, args: &SkyshotArgs) -> Result<(), String> {
    let (w, h) = (256u32, 256u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let mut failures: Vec<String> = Vec::new();

    // -- Part A: sky-disc zenith tint, no celestial pass --------------------
    // The block's value is the bare noon-zenith luma (no celestials) — the
    // baseline the sun's brightness oracle must later clearly exceed.
    let noon_zenith_luma = {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([0.0, 0.0, 0.0]);
        let vp = up_view(w, h);

        apply_sky(&mut wr, MIDNIGHT);
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let midnight = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "zenith-midnight");
        let mz = center_avg(&midnight, w, h, 8);
        if luma_f(mz) > 10.0 {
            failures.push(format!(
                "Part A: midnight zenith not black — avg rgb {mz:?} (luma {:.1})",
                luma_f(mz)
            ));
        }

        apply_sky(&mut wr, NOON);
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let noon = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "zenith-noon");
        let nz = center_avg(&noon, w, h, 8);
        // A guard against "always black passes": noon must be blue and bright.
        if nz[2] < 30.0 || nz[2] <= nz[0] {
            failures.push(format!(
                "Part A guard: noon zenith not blue/bright — avg rgb {nz:?}"
            ));
        }
        wr.destroy(gpu);
        luma_f(nz)
    };

    // -- Section 3: whole-gradient SKY_COLOR tint (permanent M11 regression) --
    // Both the zenith (up view) and the horizon (level view) must scale by the
    // requested tint. The M11 bug tinted only the horizon, leaving a blue
    // zenith; this fails if EITHER term drops `SKY_COLOR`. The attachment is
    // sRGB, so we decode the read-back bytes to linear before taking ratios.
    {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([0.0, 0.0, 0.0]);
        let tint = [0.35f32, 0.60, 0.80];
        let up = up_view(w, h);
        let horiz = horizon_view(w, h, 0.0);

        wr.set_sky_tint([1.0; 3], [1.0; 3]);
        let base_zenith = sky_center(gpu, &mut off, &mut wr, &draw, up, w, h)?;
        let base_horizon = sky_center(gpu, &mut off, &mut wr, &draw, horiz, w, h)?;

        wr.set_sky_tint(tint, tint);
        let tint_zenith = sky_center(gpu, &mut off, &mut wr, &draw, up, w, h)?;
        dump(args, &mut off, gpu, "tint-zenith");
        let tint_horizon = sky_center(gpu, &mut off, &mut wr, &draw, horiz, w, h)?;
        dump(args, &mut off, gpu, "tint-horizon");

        for (name, base, tinted) in [
            ("zenith", base_zenith, tint_zenith),
            ("horizon", base_horizon, tint_horizon),
        ] {
            for c in 0..3 {
                let bl = srgb_to_linear(base[c] / 255.0);
                let tl = srgb_to_linear(tinted[c] / 255.0);
                if bl <= 1e-4 {
                    continue; // channel carries no signal to divide
                }
                let ratio = tl / bl;
                println!(
                    "[skyshot] tint {name} ch{c}: base {:.1} tinted {:.1} -> ratio {ratio:.3} (want {:.2})",
                    base[c], tinted[c], tint[c]
                );
                if (ratio - tint[c]).abs() > 0.06 {
                    failures.push(format!(
                        "tint {name} channel {c}: ratio {ratio:.3} != requested {:.2} (SKY_COLOR dropped?)",
                        tint[c]
                    ));
                }
            }
        }
        wr.destroy(gpu);
    }

    // -- Section 2: synthetic phase + alpha + UV-orientation GPU oracle -------
    // A fresh renderer with synthetic celestial textures over a BLACK sky
    // isolates the moon-phase→atlas mapping, the OVERLAY alpha semantics, and
    // the intra-quad UV winding from the production jar. Camera faces up so the
    // parked body sits dead centre. Sun is parked below AND carries alpha 0 (the
    // discard subject). Phases 0..6 are 4×4 solids carrying a phase-identity
    // CENTRE colour (proves atlas selection; phase 0 at alpha 128). Phase 7 is
    // an 8×8 texture — grey centre (still identified by the centre sample) plus
    // four distinct 3×3 corner markers — the intra-quad orientation subject:
    // part (d) samples the four screen corners and checks the reversed moon UV
    // winding, which a solid-colour texture cannot catch.
    {
        let syn = SyntheticCel::new();
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.init_celestial(gpu, &syn.textures())?;
        wr.set_camera([0.0, 0.0, 0.0]);
        wr.set_sky_tint([0.0; 3], [0.0; 3]); // black sky
        let up = up_view(w, h);

        // Each texture's predicted centre pixel over black (SRGB→linear ×alpha
        // →SRGB), computed independently of the renderer.
        let predicted: [[f32; 3]; 8] =
            std::array::from_fn(|q| predict_moon_center(SYN_MOON_RGB[q], SYN_MOON_ALPHA[q]));

        // Base state: moon centred (angle 0), sun below (angle π), no stars/fan.
        let base = CelestialState {
            sun_angle: std::f32::consts::PI,
            moon_angle: 0.0,
            star_angle: 0.0,
            star_brightness: 0.0,
            moon_phase: 0,
            sunrise_rgba: [0.0; 4],
            rain_brightness: 1.0,
        };

        // (a) Phase selection: render every phase 0..7, identify the drawn
        // texture from the centre pixel. Catches wrong atlas order / base
        // vertex / UV selection.
        for p in 0..8u8 {
            let mut st = base;
            st.moon_phase = p;
            wr.set_celestial(st);
            off.render(gpu, Some((&mut wr, up)), &draw, CLEAR)?;
            let img = off.read_rgba(gpu)?;
            if p == 0 {
                dump(args, &mut off, gpu, "phase0");
            }
            let centre = center_avg(&img, w, h, 24);
            let centre_a = center_alpha(&img, w, h, 24);
            // Nearest predicted texture + the runner-up gap.
            let mut best = (0usize, f32::INFINITY);
            let mut second = f32::INFINITY;
            for q in 0..8 {
                let d = color_dist2(centre, predicted[q]);
                if d < best.1 {
                    second = best.1;
                    best = (q, d);
                } else if d < second {
                    second = d;
                }
            }
            // Print the full RGBA centre sample (phase 0's alpha reads ~128,
            // phases 1..7 ~255 — the OVERLAY alpha term surfaces per phase here,
            // beyond the dedicated phase-0 assertion below).
            println!(
                "[skyshot] phase {p}: centre rgba [{},{},{},{:.0}] -> texture {} (d2={:.0}, runner-up d2={:.0})",
                centre[0].round() as i32,
                centre[1].round() as i32,
                centre[2].round() as i32,
                centre_a,
                best.0,
                best.1,
                second
            );
            if best.0 as u8 != p {
                failures.push(format!(
                    "phase {p}: centre {:?} identified as texture {} (want {p})",
                    centre.map(|v| v.round() as i32),
                    best.0
                ));
            } else if best.1 > 600.0 || second < best.1 + 1500.0 {
                failures.push(format!(
                    "phase {p}: match not decisive (d2={:.0}, runner-up d2={:.0})",
                    best.1, second
                ));
            }
        }

        // (b) Discard: sun centred (angle 0) with nonzero RGB but alpha 0 must
        // NOT write — destination colour stays BLACK and destination alpha
        // stays 255. Moon parked below. The alpha is the decisive discard
        // witness: were the fully-transparent texel *not* discarded, OVERLAY's
        // alpha `src=ONE dst=ZERO` would overwrite the attachment alpha to the
        // sun's 0. The colour-black term additionally rejects a mis-specified
        // colour blend (e.g. `dst != ONE`) that would leak the warm sun RGB.
        {
            let mut st = base;
            st.sun_angle = 0.0;
            st.moon_angle = std::f32::consts::PI;
            wr.set_celestial(st);
            off.render(gpu, Some((&mut wr, up)), &draw, CLEAR)?;
            let img = off.read_rgba(gpu)?;
            let rgb = center_avg(&img, w, h, 24);
            let a = center_alpha(&img, w, h, 24);
            println!(
                "[skyshot] discard: centre rgba [{},{},{},{a:.1}] (want rgb ~0, a ~255)",
                rgb[0].round() as i32,
                rgb[1].round() as i32,
                rgb[2].round() as i32,
            );
            if luma_f(rgb) > 2.0 {
                failures.push(format!(
                    "sun discard: centre rgb {rgb:?} not black — a zero-alpha, nonzero-RGB sun must not tint the attachment"
                ));
            }
            if (a - 255.0).abs() > 2.0 {
                failures.push(format!(
                    "sun discard: centre alpha {a:.1} — a zero-alpha sun must not touch the attachment (want 255)"
                ));
            }
        }

        // (c) RGBA write/blend: the phase-0 moon (alpha 128) centred must write
        // attachment alpha ≈ 128 (OVERLAY alpha src=ONE dst=ZERO). Rejects an
        // RGB-only colour-write mask that would leave alpha at 255.
        {
            let mut st = base;
            st.moon_phase = 0;
            wr.set_celestial(st);
            off.render(gpu, Some((&mut wr, up)), &draw, CLEAR)?;
            let img = off.read_rgba(gpu)?;
            let a = center_alpha(&img, w, h, 24);
            println!("[skyshot] alpha-write: phase-0 centre alpha {a:.1} (want ~128)");
            if (a - 128.0).abs() > 4.0 {
                failures.push(format!(
                    "phase-0 alpha write: centre alpha {a:.1} != ~128 (RGB-only write mask?)"
                ));
            }
        }

        // (d) Intra-quad UV orientation: phase 7 carries four distinct corner
        // markers over a grey centre. The reversed moon UV winding
        // (`buildMoonPhases`: `(u1,v1)(u0,v1)(u0,v0)(u1,v0)`) over the
        // `(-1,0,-1)(1,0,-1)(1,0,1)(-1,0,1)` quad, through the moon's
        // `Y(-90°)·T(0,100,0)·scale(20,1,20)` pose and the up-camera, maps the
        // screen corners onto texture corners as screen-TL→tex-BR,
        // screen-TR→tex-TR, screen-BL→tex-BL, screen-BR→tex-TL. Sampling the
        // four screen corners (centre ±24 px — inside the 3×3 markers, well
        // within the ~±37 px quad) and matching each to the predicted texture
        // corner rejects a horizontal/vertical flip or a 90° rotation that the
        // centre sample cannot see. Moon centred (phase 7), sun parked below.
        {
            let mut st = base;
            st.moon_phase = 7;
            wr.set_celestial(st);
            off.render(gpu, Some((&mut wr, up)), &draw, CLEAR)?;
            let img = off.read_rgba(gpu)?;
            dump(args, &mut off, gpu, "phase7-orient");
            let (cx, cy) = (w / 2, h / 2);
            let d = 24u32;
            // (label, screen sample x, y, predicted texture-corner marker).
            let checks = [
                ("screen-TL->tex-BR", cx - d, cy - d, PHASE7_BR),
                ("screen-TR->tex-TR", cx + d, cy - d, PHASE7_TR),
                ("screen-BL->tex-BL", cx - d, cy + d, PHASE7_BL),
                ("screen-BR->tex-TL", cx + d, cy + d, PHASE7_TL),
            ];
            for (label, sx, sy, marker) in checks {
                let got = patch_avg(&img, w, h, sx, sy, 3);
                let want = predict_moon_center(marker, 255);
                let d2 = color_dist2(got, want);
                println!(
                    "[skyshot] uv-orient {label}: got {:?} want {:?} (d2={d2:.0})",
                    got.map(|v| v.round() as i32),
                    want.map(|v| v.round() as i32),
                );
                // Correct match: d² ≈ a few (NEAREST + sRGB roundtrip). Any wrong
                // permutation swaps in a marker ≥ ~50 000 away, so 900 is tight
                // yet unambiguous.
                if d2 > 900.0 {
                    failures.push(format!(
                        "uv-orientation {label}: sampled {:?} != predicted {:?} (d2={d2:.0}) — scrambled/flipped moon UV winding",
                        got.map(|v| v.round() as i32),
                        want.map(|v| v.round() as i32),
                    ));
                }
            }
        }

        wr.destroy(gpu);
    }

    // -- Section M12: analytic projected sun/moon transform + size oracle ----
    // Isolated synthetic SOLID bodies (opaque, over a BLACK sky) at a nontrivial
    // +15° body angle in the existing 256², 70°-FOV up-view, stars/fan off. The
    // four quad corners are pushed through the decompiled transform chain
    // `Y(-90)·X(angle)·T(0,100,0)·scale(size,1,size)` — reconstructed *here* in
    // f64, never via a `CelestialPass` helper — then projected through the known
    // up-camera. The read-back colored-component raster bbox + centre must match
    // to a small rasterisation tolerance. Because only the scale differs (sun
    // 30, moon 20) and the centre is scale-independent, a swapped 30/20 or a
    // flipped angle sign moves the envelope > 17 px and fails.
    {
        let solid = SolidBodies::new();
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.init_celestial(gpu, &solid.textures())?;
        wr.set_camera([0.0, 0.0, 0.0]);
        wr.set_sky_tint([0.0; 3], [0.0; 3]); // black sky isolates the body
        let up = up_view(w, h);
        let angle = M12_BODY_ANGLE_DEG.to_radians();

        // Shared projected model-centre: the quad centre (0,0,0) is untouched by
        // `scale`, so sun and moon must project to the *same* screen point — the
        // senior anchor (176.982, 128.000). The body centre before projection is
        // (-100·sinθ, 100·cosθ, 0), so +15° lands right of frame centre (positive
        // screen x).
        let center = project_up_f64(body_corner_world(0.0, 0.0, angle, M12_SUN_SIZE), w, h);
        let center_moon = project_up_f64(body_corner_world(0.0, 0.0, angle, M12_MOON_SIZE), w, h);
        println!(
            "[skyshot] M12 projected model-centre (shared): ({:.3},{:.3}) — senior ({:.3},{:.3})",
            center[0], center[1], SENIOR_CENTER[0], SENIOR_CENTER[1]
        );
        if (center[0] - center_moon[0]).abs() > 1e-6 || (center[1] - center_moon[1]).abs() > 1e-6 {
            failures.push(format!(
                "M12: sun/moon projected centres differ ({center:?} vs {center_moon:?}) — should be scale-independent"
            ));
        }
        if (center[0] - SENIOR_CENTER[0]).abs() > SENIOR_EPS
            || (center[1] - SENIOR_CENTER[1]).abs() > SENIOR_EPS
        {
            failures.push(format!(
                "M12: projected centre ({:.3},{:.3}) != senior ({:.3},{:.3})",
                center[0], center[1], SENIOR_CENTER[0], SENIOR_CENTER[1]
            ));
        }
        if center[0] <= w as f64 / 2.0 {
            failures.push(format!(
                "M12: +15° centre x {:.3} must be right of frame centre {}",
                center[0],
                w / 2
            ));
        }

        // Sun path: sun at +15°, moon parked below (θ=π → off the up-view), size 30.
        let sun_state = CelestialState {
            sun_angle: angle as f32,
            moon_angle: std::f32::consts::PI,
            star_angle: 0.0,
            star_brightness: 0.0,
            moon_phase: 0,
            sunrise_rgba: [0.0; 4],
            rain_brightness: 1.0,
        };
        failures.extend(m12_check_body(
            gpu,
            &mut off,
            &mut wr,
            &draw,
            up,
            w,
            h,
            sun_state,
            M12_SUN_SIZE,
            angle,
            SENIOR_SUN_BBOX,
            "sun",
            args,
        ));

        // Moon path: moon at +15°, sun parked below (θ=π), size 20, phase 0.
        let moon_state = CelestialState {
            sun_angle: std::f32::consts::PI,
            moon_angle: angle as f32,
            star_angle: 0.0,
            star_brightness: 0.0,
            moon_phase: 0,
            sunrise_rgba: [0.0; 4],
            rain_brightness: 1.0,
        };
        failures.extend(m12_check_body(
            gpu,
            &mut off,
            &mut wr,
            &draw,
            up,
            w,
            h,
            moon_state,
            M12_MOON_SIZE,
            angle,
            SENIOR_MOON_BBOX,
            "moon",
            args,
        ));

        wr.destroy(gpu);
    }

    // -- Section M12b: analytic sunrise-fan placement oracle -----------------
    // A fresh renderer over a BLACK sky with the sun/moon hidden (transparent
    // textures → the frag discards them) and stars off isolates the sunrise
    // fan. A controlled POSITIVE sun_angle (π/2, sin > 0) makes vanilla pick fan
    // angle 0, so the fan's bright centre lands on the −X horizon; a horizontal
    // camera looking −X frames it. The 18 logical Mth-table fan vertices are
    // reconstructed HERE (not via a `CelestialPass` helper), pushed through the
    // decompiled `X(90°)·Z(90°)·scale(1,1,alpha)` pose in f64, and projected
    // through the −X camera. The read-back warm ABOVE-HORIZON footprint (the fan
    // opens upward from its horizon centre) must match the analytic footprint —
    // vertical band [ring line .. centre] and horizontal centroid — to a small
    // raster tolerance. Because only alpha scales the fan's height and the
    // bright centre is on −X, this rejects a wrong X/Z order, a flipped sign, a
    // dropped `scale(z=alpha)`, or the fan placed on the opposite horizon. The
    // opposite +X heading is a guard: it sees only the fan's faint
    // below-horizon underside, so it must show NO comparable above-horizon warm.
    {
        let hidden = HiddenCel::new();
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.init_celestial(gpu, &hidden.textures())?;
        wr.set_camera([0.0, 0.0, 0.0]);
        wr.set_sky_tint([0.0; 3], [0.0; 3]); // black sky isolates the fan

        let alpha = M12_SUNRISE_ALPHA;
        let state = CelestialState {
            sun_angle: M12_SUNRISE_SUN_ANGLE as f32, // sin > 0 → fan angle 0 (−X)
            moon_angle: std::f32::consts::PI,
            star_angle: 0.0,
            star_brightness: 0.0,
            moon_phase: 0,
            sunrise_rgba: [
                M12_SUNRISE_RGB[0],
                M12_SUNRISE_RGB[1],
                M12_SUNRISE_RGB[2],
                alpha as f32,
            ],
            rain_brightness: 1.0,
        };

        // Analytic footprint: reconstruct the fan vertices, push through the
        // decompiled pose, project through the −X camera, keep the in-front ones
        // (all above/at the horizon on −X). The centre vertex (0,100,0) → world
        // (-100,0,0) → screen centre; the ring vertices share one screen y.
        let fan = sunrise_fan_local(alpha);
        let (mut a_min_y, mut a_max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut a_centre = [f64::NAN, f64::NAN];
        for (i, &world) in fan.iter().enumerate() {
            let (p, clip_w) = project_neg_x_f64(world, w, h);
            if i == 0 {
                a_centre = p; // the fan centre vertex
            }
            if clip_w > 1e-3 {
                a_min_y = a_min_y.min(p[1]);
                a_max_y = a_max_y.max(p[1]);
            }
        }
        // The horizon (world y=0) projects to screen centre; the fan opens
        // upward from its horizon centre, so its warm footprint is above this.
        let horizon_y = h / 2;

        // −X: render the fan, measure the warm ABOVE-HORIZON footprint.
        let neg = neg_x_view(w, h);
        wr.set_celestial(state);
        off.render(gpu, Some((&mut wr, neg)), &draw, CLEAR)?;
        let neg_img = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "m12-sunrise-negx");
        let neg_stats = warm_fan_stats(&neg_img, w, h, horizon_y);

        // +X: the opposite heading — the placement guard.
        let pos = pos_x_view(w, h);
        off.render(gpu, Some((&mut wr, pos)), &draw, CLEAR)?;
        let pos_img = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "m12-sunrise-posx");
        let pos_above = warm_fan_stats(&pos_img, w, h, horizon_y).map_or(0, |s| s.5);

        println!(
            "[skyshot] M12 sunrise: heading −X, fan angle 0, alpha {alpha}, warm colour linear {M12_SUNRISE_RGB:?}"
        );
        println!(
            "[skyshot] M12 sunrise: analytic centre ({:.3},{:.3}) band y [{:.3}..{:.3}]",
            a_centre[0], a_centre[1], a_min_y, a_max_y
        );

        let neg_count = neg_stats.map_or(0, |s| s.5);
        match neg_stats {
            None => failures.push(
                "M12 sunrise: no warm above-horizon fan on −X (fan absent / on the wrong horizon?)"
                    .into(),
            ),
            Some((min_x, min_y, max_x, max_y, centroid_x, count)) => {
                println!(
                    "[skyshot] M12 sunrise: −X actual bbox ({min_x:.1},{min_y:.1})..({max_x:.1},{max_y:.1}) centroid_x {centroid_x:.2} [{count} px]"
                );
                if count < M12_SUNRISE_MIN_PX {
                    failures.push(format!(
                        "M12 sunrise: −X warm above-horizon count {count} < {M12_SUNRISE_MIN_PX} (fan too faint / absent)"
                    ));
                }
                if (min_y - a_min_y).abs() > M12_SUNRISE_TOL {
                    failures.push(format!(
                        "M12 sunrise: −X top {min_y:.1} vs analytic ring line {a_min_y:.3} exceeds {M12_SUNRISE_TOL:.1} px (dropped scale(z=alpha)?)"
                    ));
                }
                if (max_y - a_max_y).abs() > M12_SUNRISE_TOL {
                    failures.push(format!(
                        "M12 sunrise: −X bottom {max_y:.1} vs analytic centre {a_max_y:.3} exceeds {M12_SUNRISE_TOL:.1} px"
                    ));
                }
                if (centroid_x - a_centre[0]).abs() > M12_SUNRISE_TOL {
                    failures.push(format!(
                        "M12 sunrise: −X centroid_x {centroid_x:.2} vs analytic centre x {:.3} exceeds {M12_SUNRISE_TOL:.1} px (wrong X/Z order / sign?)",
                        a_centre[0]
                    ));
                }
            }
        }

        println!(
            "[skyshot] M12 sunrise: +X guard warm above-horizon {pos_above} px (−X {neg_count} px)"
        );
        // +X may see the fan's faint below-horizon underside, but NEVER a
        // comparable above-horizon fan — that would mean the fan is (also) on
        // the opposite horizon.
        if pos_above > 20 && pos_above.saturating_mul(5) > neg_count {
            failures.push(format!(
                "M12 sunrise: +X guard shows {pos_above} warm above-horizon px vs −X {neg_count} — fan on the opposite horizon too?"
            ));
        }

        wr.destroy(gpu);
    }

    // -- Celestial bodies (need the textures) -------------------------------
    let Some(cel) = &baked.celestial else {
        return finish(
            gpu,
            off,
            &["no celestial textures in jar — cannot verify sun/moon/stars"],
        );
    };
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_celestial(gpu, &to_gpu_textures(cel))?;
    wr.set_camera([0.0, 0.0, 0.0]);

    // Section 1: deterministic star count. `buildStars` rejects a fixed subset
    // of its 1500 candidates → exactly 780 accepted / 4680 triangle indices
    // (pinned in rewo-gpu against a JOML 1.10.8 oracle). Not merely (0,1500).
    if let Some(pass) = wr.celestial_pass() {
        let q = pass.star_quad_count();
        let idx = pass.star_index_count();
        println!(
            "[skyshot] stars: {q} accepted of {} candidates ({idx} indices)",
            rewo_gpu::celestial::STAR_COUNT,
        );
        if q != 780 {
            failures.push(format!(
                "star count {q} should be exactly 780 accepted (of {} candidates)",
                rewo_gpu::celestial::STAR_COUNT
            ));
        }
        if idx != 4680 {
            failures.push(format!("star index count {idx} should be exactly 4680"));
        }
    } else {
        // The pass was just attached via `init_celestial`; a missing pass here
        // would silently skip the exact-count requirement, so fail loud.
        failures.push("celestial pass absent — cannot verify the 780/4680 star count".into());
    }

    // -- Sun: warm bright disc overhead at noon -----------------------------
    {
        let vp = up_view(w, h);
        apply_sky(&mut wr, NOON);
        wr.set_celestial(state_at(NOON));
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "sun-noon");
        // The sun disc sits at the zenith at noon; its OVERLAY-added light must
        // make the centre substantially brighter than the bare noon sky there
        // (the near-white sun saturates over the blue sky, so test brightness,
        // not hue). Would fail if the sun quad didn't render.
        let thresh = noon_zenith_luma + 50.0;
        let bright = count_pixels(&img, w, h, center_rect(w, h, 48), |p| luma(p) > thresh);
        if bright < 40 {
            failures.push(format!(
                "sun: only {bright} centre pixels brighter than {thresh:.0} at noon (bare sky {noon_zenith_luma:.0})"
            ));
        }
    }

    // -- Moon: bright disc overhead at midnight over a black sky ------------
    {
        let vp = up_view(w, h);
        apply_sky(&mut wr, MIDNIGHT);
        wr.set_celestial(state_at(MIDNIGHT));
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "moon-midnight");
        let bright = count_pixels(&img, w, h, center_rect(w, h, 48), |p| luma(p) > 60.0);
        if bright < 40 {
            failures.push(format!(
                "moon: only {bright} bright centre pixels at midnight"
            ));
        }
    }

    // -- Stars: toggling brightness adds many points -----------------------
    {
        let vp = up_view(w, h);
        apply_sky(&mut wr, MIDNIGHT);
        let mut lit = state_at(MIDNIGHT);
        lit.star_brightness = 0.5;
        wr.set_celestial(lit);
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let with_stars = off.read_rgba(gpu)?;
        dump(args, &mut off, gpu, "stars-on");

        let mut dark = state_at(MIDNIGHT);
        dark.star_brightness = 0.0;
        wr.set_celestial(dark);
        off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
        let no_stars = off.read_rgba(gpu)?;

        // Count pixels that brightened when stars turned on, ignoring the moon
        // region (unchanged between the two frames anyway).
        let mut added = 0usize;
        for i in (0..with_stars.len()).step_by(4) {
            let a = luma([with_stars[i], with_stars[i + 1], with_stars[i + 2]]);
            let b = luma([no_stars[i], no_stars[i + 1], no_stars[i + 2]]);
            if a - b > 8.0 {
                added += 1;
            }
        }
        if added < 30 {
            failures.push(format!(
                "stars: only {added} pixels brightened when stars enabled"
            ));
        }
    }

    // The sunrise fan is verified analytically in Section M12b above (an
    // isolated renderer with a controlled heading/colour/alpha), not by a
    // heading sweep here.

    wr.destroy(gpu);
    let fails: Vec<&str> = failures.iter().map(|s| s.as_str()).collect();
    finish(gpu, off, &fails)
}

fn finish(gpu: &mut Gpu, mut off: Offscreen, failures: &[&str]) -> Result<(), String> {
    off.destroy(gpu);
    if failures.is_empty() {
        println!("[skyshot] CHECK OK — sky zenith tint + sun/moon/stars/sunrise verified");
        Ok(())
    } else {
        for f in failures {
            println!("[skyshot] FAIL {f}");
        }
        Err(format!(
            "skyshot check: {} property failures",
            failures.len()
        ))
    }
}

// -- state + camera -----------------------------------------------------------

/// Feed the renderer the day/night sky tint + lightmap for a tick (drives the
/// sky-disc gradient the zenith check reads).
fn apply_sky(wr: &mut WorldRenderer, tick: i64) {
    let d = daylight::sky_lighting(tick);
    wr.set_sky_tint(d.sky_color, d.fog_color);
    wr.set_lightmap(d.light_factor, d.light_color);
}

/// Build the GPU celestial state from the timeline: angles → radians, sunrise
/// ARGB → linear rgb + straight alpha.
fn state_at(tick: i64) -> CelestialState {
    let c = celestial::celestial_at(tick);
    let argb = c.sunrise_sunset_color;
    let ch = |sh: u32| ((argb >> sh) & 0xFF) as f32 / 255.0;
    CelestialState {
        sun_angle: c.sun_angle_rad(),
        moon_angle: c.moon_angle_rad(),
        star_angle: c.star_angle_rad(),
        star_brightness: c.star_brightness,
        moon_phase: c.moon_phase,
        sunrise_rgba: [
            srgb_to_linear(ch(16)),
            srgb_to_linear(ch(8)),
            srgb_to_linear(ch(0)),
            ch(24),
        ],
        rain_brightness: 1.0,
    }
}

fn to_gpu_textures(cel: &assets::CelestialTextures) -> CelestialTextures<'_> {
    fn img(i: &assets::DecodedImage) -> CelestialImage<'_> {
        CelestialImage {
            rgba: &i.rgba,
            w: i.w,
            h: i.h,
        }
    }
    CelestialTextures {
        sun: img(&cel.sun),
        moons: std::array::from_fn(|k| img(&cel.moons[k])),
    }
}

fn up_view(w: u32, h: u32) -> [[f32; 4]; 4] {
    view(w, h, Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, -1.0))
}

fn horizon_view(w: u32, h: u32, azimuth: f32) -> [[f32; 4]; 4] {
    let dir = Vec3::new(azimuth.cos(), 0.0, azimuth.sin());
    view(w, h, dir, Vec3::Y)
}

/// Horizontal camera looking exactly −X (the M12b sunrise fan's heading). Exact
/// dir avoids any azimuth round-off vs the analytic `project_neg_x_f64`.
fn neg_x_view(w: u32, h: u32) -> [[f32; 4]; 4] {
    view(w, h, Vec3::new(-1.0, 0.0, 0.0), Vec3::Y)
}

/// Horizontal camera looking exactly +X — the opposite-heading placement guard.
fn pos_x_view(w: u32, h: u32) -> [[f32; 4]; 4] {
    view(w, h, Vec3::new(1.0, 0.0, 0.0), Vec3::Y)
}

fn view(w: u32, h: u32, dir: Vec3, up: Vec3) -> [[f32; 4]; 4] {
    let eye = Vec3::ZERO;
    let view = Mat4::look_to_rh(eye, dir, up);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        w as f32 / h as f32,
        0.05,
    ));
    (proj * view).to_cols_array_2d()
}

// -- pixel analysis -----------------------------------------------------------

fn luma(p: [u8; 3]) -> f32 {
    luma_f([p[0] as f32, p[1] as f32, p[2] as f32])
}

fn luma_f(p: [f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// Average RGB over a centred square of half-size `w/frac`.
fn center_avg(img: &[u8], w: u32, h: u32, frac: u32) -> [f32; 3] {
    let (cx, cy) = (w / 2, h / 2);
    let r = (w / frac).max(1);
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for y in cy.saturating_sub(r)..(cy + r).min(h) {
        for x in cx.saturating_sub(r)..(cx + r).min(w) {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                sum[c] += img[i + c] as f64;
            }
            n += 1.0;
        }
    }
    [
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    ]
}

/// Average RGB over a square of half-size `r` centred at screen `(x,y)` — the
/// offset variant of `center_avg`, used to sample the four quad corners for the
/// intra-quad UV-orientation oracle.
fn patch_avg(img: &[u8], w: u32, h: u32, x: u32, y: u32, r: u32) -> [f32; 3] {
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for py in y.saturating_sub(r)..(y + r + 1).min(h) {
        for px in x.saturating_sub(r)..(x + r + 1).min(w) {
            let i = ((py * w + px) * 4) as usize;
            for c in 0..3 {
                sum[c] += img[i + c] as f64;
            }
            n += 1.0;
        }
    }
    [
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    ]
}

/// A centred square rect `(x0,y0,x1,y1)` of half-size `r`.
fn center_rect(w: u32, h: u32, r: u32) -> (u32, u32, u32, u32) {
    let (cx, cy) = (w / 2, h / 2);
    (
        cx.saturating_sub(r),
        cy.saturating_sub(r),
        (cx + r).min(w),
        (cy + r).min(h),
    )
}

fn count_pixels(
    img: &[u8],
    w: u32,
    _h: u32,
    rect: (u32, u32, u32, u32),
    pred: impl Fn([u8; 3]) -> bool,
) -> usize {
    let mut n = 0;
    for y in rect.1..rect.3 {
        for x in rect.0..rect.2 {
            let i = ((y * w + x) * 4) as usize;
            if pred([img[i], img[i + 1], img[i + 2]]) {
                n += 1;
            }
        }
    }
    n
}

/// Render the sky (optionally with celestials) through `wr` and return the
/// centre-patch average RGB. Used by the tint regression (Section 3).
fn sky_center(
    gpu: &Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    vp: [[f32; 4]; 4],
    w: u32,
    h: u32,
) -> Result<[f32; 3], String> {
    off.render(gpu, Some((wr, vp)), draw, CLEAR)?;
    Ok(center_avg(&off.read_rgba(gpu)?, w, h, 16))
}

/// Average of the (linear) alpha byte over a centred square of half-size
/// `w/frac`. Alpha in `R8G8B8A8_SRGB` is stored straight, so this is the raw
/// attachment coverage — the subject of the discard / write-mask oracle.
fn center_alpha(img: &[u8], w: u32, h: u32, frac: u32) -> f32 {
    let (cx, cy) = (w / 2, h / 2);
    let r = (w / frac).max(1);
    let (mut sum, mut n) = (0f64, 0f64);
    for y in cy.saturating_sub(r)..(cy + r).min(h) {
        for x in cx.saturating_sub(r)..(cx + r).min(w) {
            let i = ((y * w + x) * 4) as usize;
            sum += img[i + 3] as f64;
            n += 1.0;
        }
    }
    (sum / n) as f32
}

/// Linear → sRGB encode (the transfer function the `R8G8B8A8_SRGB` attachment
/// applies on store). Inverse of `entities::srgb_to_linear`.
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The synthetic-moon centre pixel over a black sky, derived independently of
/// the renderer: the SRGB atlas linearises the texel, OVERLAY multiplies by the
/// (linear) alpha over black, the SRGB attachment re-encodes on store.
fn predict_moon_center(rgb: [u8; 3], alpha: u8) -> [f32; 3] {
    let a = alpha as f32 / 255.0;
    std::array::from_fn(|c| {
        let lin = srgb_to_linear(rgb[c] as f32 / 255.0) * a;
        (linear_to_srgb(lin) * 255.0).clamp(0.0, 255.0)
    })
}

fn color_dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum()
}

// -- synthetic celestial textures (Section 2 oracle) --------------------------

/// Eight mutually-distinct solid moon colours (chosen so their predicted centre
/// pixels stay well separated even after phase 0's alpha-128 dimming — minimum
/// pairwise d² ≈ 5100).
const SYN_MOON_RGB: [[u8; 3]; 8] = [
    [230, 40, 40],   // 0 red
    [40, 210, 40],   // 1 green
    [40, 60, 230],   // 2 blue
    [230, 210, 40],  // 3 yellow
    [220, 40, 200],  // 4 magenta
    [40, 210, 210],  // 5 cyan
    [240, 140, 30],  // 6 orange
    [180, 180, 180], // 7 grey
];
/// Phase 0 alpha 128, phases 1..7 alpha 255 (the alpha oracle).
const SYN_MOON_ALPHA: [u8; 8] = [128, 255, 255, 255, 255, 255, 255, 255];
/// The sun: nonzero RGB, alpha 0 — the discard subject.
const SYN_SUN_RGB: [u8; 3] = [255, 200, 120];

/// Phase 7 is the intra-quad UV-orientation subject: an 8×8 texture (vs the 4×4
/// solids), grey in the centre with four distinct opaque 3×3 markers stamped in
/// the texture corners. A solid colour is flip/rotation-invariant at the centre;
/// the corner markers are not, so they catch a scrambled or reversed winding.
const PHASE7_DIM: u32 = 8;
/// Marker colours by *texture* corner, chosen mutually far apart (min pairwise
/// d² ≈ 50 625) so any wrong winding permutation is unmissable.
const PHASE7_TL: [u8; 3] = [255, 30, 30]; // texture top-left     — red
const PHASE7_TR: [u8; 3] = [30, 255, 30]; // texture top-right    — green
const PHASE7_BL: [u8; 3] = [30, 30, 255]; // texture bottom-left  — blue
const PHASE7_BR: [u8; 3] = [255, 255, 30]; // texture bottom-right — yellow

/// Build phase 7's 8×8 orientation texture: grey (phase 7's identity colour)
/// base with the four opaque 3×3 corner markers stamped in. Texel `[y][x]` has
/// row 0 = top (`v0`) and col 0 = left (`u0`), matching the atlas blit's
/// row-major copy, so the corner names below are the texture's own corners.
fn synth_phase7() -> Vec<u8> {
    let dim = PHASE7_DIM as usize;
    let mut v = vec![0u8; dim * dim * 4];
    let put = |v: &mut [u8], x: usize, y: usize, rgb: [u8; 3]| {
        let i = (y * dim + x) * 4;
        v[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    };
    for y in 0..dim {
        for x in 0..dim {
            put(&mut v, x, y, SYN_MOON_RGB[7]); // grey base
        }
    }
    let stamp = |v: &mut [u8], cols: [usize; 2], rows: [usize; 2], rgb: [u8; 3]| {
        for y in rows[0]..rows[1] {
            for x in cols[0]..cols[1] {
                put(v, x, y, rgb);
            }
        }
    };
    stamp(&mut v, [0, 3], [0, 3], PHASE7_TL);
    stamp(&mut v, [5, 8], [0, 3], PHASE7_TR);
    stamp(&mut v, [0, 3], [5, 8], PHASE7_BL);
    stamp(&mut v, [5, 8], [5, 8], PHASE7_BR);
    v
}

/// Owned RGBA buffers for the synthetic sun + 8 moons; `textures()` borrows them
/// into a `CelestialTextures` for `init_celestial`. Phases 0..6 + the sun are
/// 4×4 solids; phase 7 is the 8×8 orientation texture (`moon_dims` carries the
/// per-moon size).
struct SyntheticCel {
    sun: Vec<u8>,
    moons: [Vec<u8>; 8],
    moon_dims: [u32; 8],
    dim: u32,
}

impl SyntheticCel {
    fn new() -> Self {
        let dim = 4u32;
        let fill = |dim: u32, rgb: [u8; 3], a: u8| -> Vec<u8> {
            let mut v = Vec::with_capacity((dim * dim * 4) as usize);
            for _ in 0..dim * dim {
                v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
            }
            v
        };
        let mut moons: [Vec<u8>; 8] =
            std::array::from_fn(|i| fill(dim, SYN_MOON_RGB[i], SYN_MOON_ALPHA[i]));
        let mut moon_dims = [dim; 8];
        moons[7] = synth_phase7();
        moon_dims[7] = PHASE7_DIM;
        Self {
            sun: fill(dim, SYN_SUN_RGB, 0),
            moons,
            moon_dims,
            dim,
        }
    }

    fn textures(&self) -> CelestialTextures<'_> {
        CelestialTextures {
            sun: CelestialImage {
                rgba: &self.sun,
                w: self.dim,
                h: self.dim,
            },
            moons: std::array::from_fn(|i| CelestialImage {
                rgba: &self.moons[i],
                w: self.moon_dims[i],
                h: self.moon_dims[i],
            }),
        }
    }
}

// -- M12 analytic projected-body transform (independent of the renderer) ------

/// Rotate a point about +X by `a` (radians): the `X(angle)` in the chain.
fn rot_x(p: [f64; 3], a: f64) -> [f64; 3] {
    let (s, c) = a.sin_cos();
    [p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]]
}

/// Rotate a point about +Y by `a` (radians): the base `Y(-90°)`.
fn rot_y(p: [f64; 3], a: f64) -> [f64; 3] {
    let (s, c) = a.sin_cos();
    [c * p[0] + s * p[2], p[1], -s * p[0] + c * p[2]]
}

/// One celestial-quad corner `(sx,0,sz)` (sx,sz ∈ {−1,+1}, or 0 for the centre)
/// pushed through the decompiled sun/moon chain
/// `Y(-90°)·X(angle)·T(0,100,0)·scale(size,1,size)`, into world space. Applied
/// right-to-left to the point (== `M·v`), reconstructed here rather than via any
/// `CelestialPass` helper. The quad lives in the XZ plane, so `scale` touches
/// x/z and `T` lifts y from 0 → 100.
fn body_corner_world(sx: f64, sz: f64, angle_rad: f64, size: f64) -> [f64; 3] {
    let scaled = [size * sx, 0.0, size * sz]; // scale(size,1,size)
    let lifted = [scaled[0], scaled[1] + 100.0, scaled[2]]; // T(0,100,0)
    let turned = rot_x(lifted, angle_rad); // X(angle)
    rot_y(turned, (-90.0f64).to_radians()) // Y(-90°)
}

/// Project a world point through the known up-camera (`up_view`) in f64: the RH
/// look-to basis (dir +Y, up −Z), `perspective_reverse_z(70°, w/h, near)`, and
/// the celestial pass's y-flipped viewport (`y=h, height=-h`). `near` only sets
/// clip.z, so it is irrelevant to the x/y projection and omitted. Returns
/// framebuffer pixel coords (origin top-left, matching `read_rgba`).
fn project_up_f64(world: [f64; 3], w: u32, h: u32) -> [f64; 2] {
    // View basis for look_to_rh(eye=0, dir=(0,1,0), up=(0,0,-1)):
    //   s=(-1,0,0), u=(0,0,-1), f=(0,1,0); view = (s·p, u·p, -f·p).
    let vx = -world[0];
    let vy = -world[2];
    let vz = -world[1];
    let fov = 70.0f64.to_radians();
    let f = 1.0 / (fov * 0.5).tan();
    let aspect = w as f64 / h as f64;
    let clip_w = -vz; // perspective_reverse_z: clip.w = -view_z
    let ndc_x = (f / aspect * vx) / clip_w;
    let ndc_y = (f * vy) / clip_w;
    let sx = (ndc_x * 0.5 + 0.5) * w as f64;
    let sy = h as f64 - (ndc_y * 0.5 + 0.5) * h as f64; // viewport y-flip
    [sx, sy]
}

// -- M12b analytic sunrise fan (independent of the renderer) -------------------

/// Rotate a point about +Z by `a` (radians): the `Z(angle+90°)` in the fan pose.
fn rot_z(p: [f64; 3], a: f64) -> [f64; 3] {
    let (s, c) = a.sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]]
}

/// One `Mth` sine-table entry `SIN[rawIndex & 65535] = (float)Math.sin(idx /
/// SCALE)`, reconstructed here from the decompile (std `sin` matches Java's
/// `Math.sin` closely enough — the vertical fan bound the check reads is
/// independent of the sub-ULP table value anyway, since the ring's `cos`
/// cancels in the projection).
fn mth_sin_table(raw_index: i64) -> f32 {
    let idx = raw_index & 65535;
    (idx as f64 / MTH_SIN_SCALE).sin() as f32
}

/// `Mth.sin(float)` — table lookup with the decompile's `(long)` truncation.
fn mth_sin(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE) as i64)
}

/// `Mth.cos(float)` — the same table offset a quarter turn (16384 entries).
fn mth_cos(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE + 16384.0) as i64)
}

/// The 18 `buildSunriseFan` logical vertices — centre `(0,100,0)` then the
/// 17-point ring `(sin·120, cos·120, -cos·40)` with `angle = i·(2π)/16` and the
/// `Mth` sine table — reconstructed here (NOT via `rewo-gpu`), each pushed
/// through the decompiled fan pose `X(90°)·Z(90°)·scale(1,1,alpha)` into world
/// space (fan angle 0 → the `Z(angle+90°)` term is `Z(90°)`). The pose is
/// applied right-to-left (`M·v`): scale, then `Z(90°)`, then `X(90°)`.
fn sunrise_fan_local(alpha: f64) -> [[f64; 3]; 18] {
    let tau = (std::f64::consts::PI * 2.0) as f32; // (float)(Math.PI * 2.0)
    let mut world = [[0.0f64; 3]; 18];
    let pose = |local: [f32; 3]| -> [f64; 3] {
        let v = [local[0] as f64, local[1] as f64, local[2] as f64];
        let scaled = [v[0], v[1], alpha * v[2]]; // scale(1, 1, alpha)
        let turned = rot_z(scaled, 90.0f64.to_radians()); // Z(angle+90°), angle=0
        rot_x(turned, 90.0f64.to_radians()) // X(90°)
    };
    world[0] = pose([0.0, 100.0, 0.0]); // fan centre
    for i in 0..=16 {
        let angle = i as f32 * tau / 16.0;
        let s = mth_sin(angle);
        let c = mth_cos(angle);
        world[i + 1] = pose([s * 120.0, c * 120.0, -c * 40.0]);
    }
    world
}

/// Project a world point through the horizontal −X camera (`neg_x_view`) in f64,
/// returning `(screen_xy, clip_w)`. `look_to_rh(eye=0, dir=(-1,0,0), up=(0,1,0))`
/// has basis `s=(0,0,-1)`, `u=(0,1,0)`, `f=(-1,0,0)`, so `view=(-p_z, p_y, p_x)`
/// and `clip_w = -view_z = -p_x`. `clip_w > 0` ⇔ the point is in front. Same
/// `perspective_reverse_z(70°)` + y-flipped viewport as `project_up_f64`.
fn project_neg_x_f64(world: [f64; 3], w: u32, h: u32) -> ([f64; 2], f64) {
    let vx = -world[2];
    let vy = world[1];
    let vz = world[0];
    let fov = 70.0f64.to_radians();
    let f = 1.0 / (fov * 0.5).tan();
    let aspect = w as f64 / h as f64;
    let clip_w = -vz; // = -world_x
    let ndc_x = (f / aspect * vx) / clip_w;
    let ndc_y = (f * vy) / clip_w;
    let sx = (ndc_x * 0.5 + 0.5) * w as f64;
    let sy = h as f64 - (ndc_y * 0.5 + 0.5) * h as f64; // viewport y-flip
    ([sx, sy], clip_w)
}

/// Warm above-horizon footprint of a read-back frame: over the pure-black,
/// bodies-hidden sky a fragment is "warm fan" when red is clearly present and
/// clearly exceeds blue (`M12_SUNRISE_RGB` has r ≫ b, so this holds even at the
/// faded ring edge). Restricted to rows `y ≤ y_limit` (the fan opens *upward*
/// from its horizon centre, and this excludes the fan's below-horizon underside
/// that the +X guard would otherwise pick up). Returns `(min_x, min_y, max_x,
/// max_y, centroid_x, count)` using the same span convention as `colored_bbox`
/// (min = raw index, max = index+1, centroid at pixel centres) so the measured
/// bounds compare directly against the analytic projected coordinates.
fn warm_fan_stats(
    img: &[u8],
    w: u32,
    h: u32,
    y_limit: u32,
) -> Option<(f64, f64, f64, f64, f64, usize)> {
    let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let (mut sum_x, mut n) = (0f64, 0usize);
    let y_top = (y_limit + 1).min(h);
    for y in 0..y_top {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (r, b) = (img[i] as i32, img[i + 2] as i32);
            if r > 12 && r - b > 8 {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                sum_x += x as f64 + 0.5;
                n += 1;
            }
        }
    }
    (n > 0).then_some((
        minx as f64,
        miny as f64,
        maxx as f64 + 1.0,
        maxy as f64 + 1.0,
        sum_x / n as f64,
        n,
    ))
}

/// The colored-component raster bbox `(min_x,min_y,max_x,max_y)` in **pixel
/// indices** plus the pixel count. A pixel is "body" when its max channel ≥
/// `thresh` (excludes the pure-black clear/sky).
fn colored_bbox(img: &[u8], w: u32, h: u32, thresh: u8) -> Option<(f64, f64, f64, f64, usize)> {
    let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut n = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let m = img[i].max(img[i + 1]).max(img[i + 2]);
            if m >= thresh {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                n += 1;
            }
        }
    }
    (n > 0).then_some((minx as f64, miny as f64, maxx as f64, maxy as f64, n))
}

/// Render one isolated body, then compare its read-back raster bbox + centre to
/// the analytic projection of the four quad corners for `size`. Returns the
/// failure list (empty on success). Also pins the analytic envelope against the
/// senior double-precision cross-check, so both the derivation and the render
/// are verified.
#[allow(clippy::too_many_arguments)]
fn m12_check_body(
    gpu: &Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    up: [[f32; 4]; 4],
    w: u32,
    h: u32,
    state: CelestialState,
    size: f64,
    angle: f64,
    senior_bbox: [f64; 4],
    name: &str,
    args: &SkyshotArgs,
) -> Vec<String> {
    let mut fails = Vec::new();

    // Analytic envelope from the four projected corners.
    let (mut minx, mut miny, mut maxx, mut maxy) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for &(sx, sz) in &[(1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let p = project_up_f64(body_corner_world(sx, sz, angle, size), w, h);
        minx = minx.min(p[0]);
        maxx = maxx.max(p[0]);
        miny = miny.min(p[1]);
        maxy = maxy.max(p[1]);
    }
    let analytic = [minx, miny, maxx, maxy];
    let a_center = [(minx + maxx) * 0.5, (miny + maxy) * 0.5];
    for (k, label) in ["min_x", "min_y", "max_x", "max_y"].iter().enumerate() {
        if (analytic[k] - senior_bbox[k]).abs() > SENIOR_EPS {
            fails.push(format!(
                "M12 {name}: analytic {label} {:.3} != senior {:.3}",
                analytic[k], senior_bbox[k]
            ));
        }
    }

    // Render the isolated body and read it back.
    wr.set_celestial(state);
    if let Err(e) = off.render(gpu, Some((&mut *wr, up)), draw, CLEAR) {
        fails.push(format!("M12 {name}: render: {e}"));
        return fails;
    }
    let img = match off.read_rgba(gpu) {
        Ok(i) => i,
        Err(e) => {
            fails.push(format!("M12 {name}: read: {e}"));
            return fails;
        }
    };
    dump(args, off, gpu, &format!("m12-{name}"));

    let Some((mnx, mny, mxx, mxy, npx)) = colored_bbox(&img, w, h, M12_BODY_MASK) else {
        fails.push(format!(
            "M12 {name}: no colored body pixels found (body did not render?)"
        ));
        return fails;
    };
    // Pixel index i covers [i, i+1), so the covered span is [min, max+1].
    let measured = [mnx, mny, mxx + 1.0, mxy + 1.0];
    let m_center = [
        (measured[0] + measured[2]) * 0.5,
        (measured[1] + measured[3]) * 0.5,
    ];

    println!(
        "[skyshot] M12 {name}: expected bbox ({:.3},{:.3})..({:.3},{:.3}) centre ({:.3},{:.3})",
        analytic[0], analytic[1], analytic[2], analytic[3], a_center[0], a_center[1]
    );
    println!(
        "[skyshot] M12 {name}: actual   bbox ({:.1},{:.1})..({:.1},{:.1}) centre ({:.1},{:.1}) [{npx} px]",
        measured[0], measured[1], measured[2], measured[3], m_center[0], m_center[1]
    );

    for (k, label) in ["min_x", "min_y", "max_x", "max_y"].iter().enumerate() {
        if (measured[k] - analytic[k]).abs() > M12_RASTER_TOL {
            fails.push(format!(
                "M12 {name}: raster {label} {:.1} vs analytic {:.3} exceeds {:.1} px",
                measured[k], analytic[k], M12_RASTER_TOL
            ));
        }
    }
    for (c, label) in ["x", "y"].iter().enumerate() {
        if (m_center[c] - a_center[c]).abs() > M12_RASTER_TOL {
            fails.push(format!(
                "M12 {name}: raster centre {label} {:.1} vs analytic {:.3} exceeds {:.1} px",
                m_center[c], a_center[c], M12_RASTER_TOL
            ));
        }
    }
    fails
}

// -- M12 synthetic SOLID bodies (opaque, distinct sun/moon colours) -----------

/// Warm opaque sun / cool opaque moon for the M12 transform oracle. Opaque
/// (alpha 255 → no frag discard) so the projected quad rasterises as a solid
/// colored region; the two hues just keep the eyeball dumps legible.
const M12_SUN_RGB: [u8; 3] = [255, 196, 92];
const M12_MOON_RGB: [u8; 3] = [150, 190, 255];

/// Owned RGBA buffers for the solid sun + 8 (identical) solid moons.
struct SolidBodies {
    sun: Vec<u8>,
    moons: [Vec<u8>; 8],
    dim: u32,
}

impl SolidBodies {
    fn new() -> Self {
        let dim = 4u32;
        let fill = |rgb: [u8; 3]| -> Vec<u8> {
            let mut v = Vec::with_capacity((dim * dim * 4) as usize);
            for _ in 0..dim * dim {
                v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            v
        };
        Self {
            sun: fill(M12_SUN_RGB),
            moons: std::array::from_fn(|_| fill(M12_MOON_RGB)),
            dim,
        }
    }

    fn textures(&self) -> CelestialTextures<'_> {
        CelestialTextures {
            sun: CelestialImage {
                rgba: &self.sun,
                w: self.dim,
                h: self.dim,
            },
            moons: std::array::from_fn(|i| CelestialImage {
                rgba: &self.moons[i],
                w: self.dim,
                h: self.dim,
            }),
        }
    }
}

// -- M12b hidden bodies (fully transparent → the frag discards them) ----------

/// Fully-transparent sun + 8 moons for the M12b sunrise oracle: every texel is
/// RGBA 0, so `out_color.a == 0` in `celestial.frag` and the disc is discarded
/// — the sun/moon never touch the attachment, leaving the sunrise fan alone
/// over a black sky. `init_celestial` still needs *some* textures.
struct HiddenCel {
    clear: Vec<u8>,
    dim: u32,
}

impl HiddenCel {
    fn new() -> Self {
        let dim = 4u32;
        Self {
            clear: vec![0u8; (dim * dim * 4) as usize],
            dim,
        }
    }

    fn textures(&self) -> CelestialTextures<'_> {
        let img = || CelestialImage {
            rgba: &self.clear,
            w: self.dim,
            h: self.dim,
        };
        CelestialTextures {
            sun: img(),
            moons: std::array::from_fn(|_| img()),
        }
    }
}

fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn dump(args: &SkyshotArgs, off: &mut Offscreen, gpu: &Gpu, name: &str) {
    if let Some(dir) = &args.out_dir {
        // Best-effort eyeball artifact — re-reads the last rendered frame.
        let _ = off.save_png(gpu, &dir.join(format!("{name}.png")));
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
