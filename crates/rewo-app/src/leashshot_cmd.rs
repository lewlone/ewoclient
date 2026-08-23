//! `rewo leashshot --check` — the serverless gate for the leash rope (M170).
//!
//! It renders `leash::build_ribbon`'s output through the production
//! `WorldRenderer::draw_leash` pass into an offscreen target and reads the
//! pixels back, so it grades the whole GPU path — the pipeline, the vertex
//! format, the depth/blend config — not just the geometry (which the module's
//! own unit tests pin). The CPU gather (`collect_leashes`) is live-only and is
//! graded by `live --render-check`'s r60.
//!
//! Five witnesses, each with a control that must find nothing or the opposite:
//! an empty frame draws no rope; a level rope draws a thin brown line; a slack
//! rope sags below a taut one; the alternating segments carry two brown shades;
//! and the light fades along a rope lit at one end.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};

use rewo_data::{assets, DataPaths};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;
const LIT: [f32; 3] = [1.0, 1.0, 1.0];
const DARK: [f32; 3] = [0.0, 0.0, 0.0];

#[derive(ClapArgs)]
pub struct LeashshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Assert the leash properties and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Optional: also dump the rendered frames here (eyeball artifact).
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Bypass Vulkan validation layers.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: LeashshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[leashshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err("leashshot check: Vulkan validation requested but not active — \
            install the Vulkan SDK, or pass --no-validation"
            .into());
    }
    run_check(&mut gpu, &baked, &args)
}

/// Camera looking straight down +Z from `(0, 0, -dist)`, framing a rope drawn
/// around the world origin — the exact `look_to_rh` + `perspective_reverse_z`
/// convention the live path uses (`eye_view` in `live_cmd`).
fn view_proj(dist: f32) -> [[f32; 4]; 4] {
    let eye = Vec3::new(0.0, 0.0, -dist);
    let view = Mat4::look_to_rh(eye, Vec3::Z, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    (proj * view).to_cols_array_2d()
}

/// A pixel is "rope" if it is brown (r > g >= b) and clearly above the black
/// clear. The base linear reds are ~54/255 (bright) and ~25/255 (dim), so the
/// threshold sits below the dim segment yet well above clear.
fn is_rope(p: [u8; 4]) -> bool {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    r > 8 && r > g && g >= b
}

/// Every rope pixel's `(x, y, red)`.
fn rope_pixels(rgba: &[u8]) -> Vec<(u32, u32, u8)> {
    let mut v = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let px = [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
            if is_rope(px) {
                v.push((x, y, px[0]));
            }
        }
    }
    v
}

fn run_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    args: &LeashshotArgs,
) -> Result<(), String> {
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let mut off = Offscreen::new(gpu, W, H)?;
    let mut fails: Vec<String> = Vec::new();

    // A closure that renders `verts` and reads the frame back.
    let mut frame = |gpu: &mut Gpu,
                     off: &mut Offscreen,
                     verts: &[rewo_gpu::leash::LeashVertex],
                     dist: f32|
     -> Result<Vec<u8>, String> {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([0.0, 0.0, -dist]);
        wr.set_leash(verts);
        off.render(gpu, Some((&mut wr, view_proj(dist))), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };
    let dump = |off: &mut Offscreen, gpu: &mut Gpu, name: &str| {
        if let Some(dir) = &args.out_dir {
            let _ = off.save_png(gpu, &dir.join(format!("leash-{name}.png")));
        }
    };

    use rewo_gpu::leash::build_ribbon;

    // -- g0: an empty frame draws no rope ---------------------------------
    let empty = frame(gpu, &mut off, &[], 5.0)?;
    dump(&mut off, gpu, "empty");
    let n_empty = rope_pixels(&empty).len();
    if n_empty > 20 {
        fails.push(format!("g0 empty frame has {n_empty} rope pixels (want ~0)"));
    }

    // -- g1: a level rope draws a thin brown line -------------------------
    let level = build_ribbon([-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], false, LIT, LIT);
    let f1 = frame(gpu, &mut off, &level, 5.0)?;
    dump(&mut off, gpu, "level");
    let p1 = rope_pixels(&f1);
    if p1.len() < 200 {
        fails.push(format!("g1 level rope only {} rope pixels (want a line)", p1.len()));
    } else {
        let xs: Vec<u32> = p1.iter().map(|&(x, _, _)| x).collect();
        let ys: Vec<u32> = p1.iter().map(|&(_, y, _)| y).collect();
        let (xspan, yspan) = (span(&xs), span(&ys));
        // A horizontal rope: much wider than it is tall.
        if xspan < yspan * 3 {
            fails.push(format!(
                "g1 rope not a horizontal line — x span {xspan}, y span {yspan}"
            ));
        }
    }

    // -- g2: a slack rope sags below a taut one ---------------------------
    // A rope to a point below: slack curves under the straight interpolation.
    let start = [-2.0, 1.5, 0.0];
    let end = [2.0, -1.5, 0.0];
    let slack = build_ribbon(start, end, true, LIT, LIT);
    let taut = build_ribbon(start, end, false, LIT, LIT);
    let fs = frame(gpu, &mut off, &slack, 5.0)?;
    dump(&mut off, gpu, "slack");
    let ft = frame(gpu, &mut off, &taut, 5.0)?;
    dump(&mut off, gpu, "taut");
    // Near the horizontal centre column (world x = 0 -> screen centre), the
    // slack rope's pixels reach a LARGER screen y (lower on screen).
    let mid_low = |px: &[(u32, u32, u8)]| -> Option<u32> {
        px.iter()
            .filter(|&&(x, _, _)| (x as i32 - W as i32 / 2).abs() < 12)
            .map(|&(_, y, _)| y)
            .max()
    };
    match (mid_low(&rope_pixels(&fs)), mid_low(&rope_pixels(&ft))) {
        (Some(sl), Some(tl)) => {
            if sl <= tl + 2 {
                fails.push(format!(
                    "g2 slack rope did not sag — slack low y {sl}, taut low y {tl}"
                ));
            }
        }
        other => fails.push(format!("g2 could not find both ropes at centre: {other:?}")),
    }

    // -- g3: alternating segments carry two brown shades ------------------
    // On the level rope, the reds cluster into a bright and a dimmed value
    // (the 0.7 colorModifier). Assert the spread of distinct reds spans it.
    let reds: Vec<u8> = p1.iter().map(|&(_, _, r)| r).collect();
    if let (Some(&lo), Some(&hi)) = (reds.iter().min(), reds.iter().max()) {
        // Bright/dim ratio is 1/0.7 ~= 1.43; even after sRGB curvature the two
        // shades differ by well over a threshold. Guard against a flat rope.
        if hi < lo + 8 {
            fails.push(format!(
                "g3 no alternating dim — reds span only [{lo}, {hi}]"
            ));
        }
    } else {
        fails.push("g3 no rope reds to inspect".into());
    }

    // -- g4: light fades along a rope lit at one end ----------------------
    let faded = build_ribbon([-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], false, LIT, DARK);
    let f4 = frame(gpu, &mut off, &faded, 5.0)?;
    dump(&mut off, gpu, "faded");
    let p4 = rope_pixels(&f4);
    // World x = -2 is the lit end. Which screen side that is depends on the
    // look_to_rh handedness, so compare the two halves and demand a strong
    // gradient in ONE direction rather than assuming which.
    let mut left: Vec<u8> = Vec::new();
    let mut right: Vec<u8> = Vec::new();
    for &(x, _, r) in &p4 {
        if x < W / 2 {
            left.push(r);
        } else {
            right.push(r);
        }
    }
    let avg = |v: &[u8]| if v.is_empty() { 0.0 } else { v.iter().map(|&r| r as f32).sum::<f32>() / v.len() as f32 };
    let (la, ra) = (avg(&left), avg(&right));
    if (la - ra).abs() < 10.0 {
        fails.push(format!(
            "g4 light did not fade — left avg red {la:.1}, right avg red {ra:.1}"
        ));
    }

    let total = 5;
    let passed = total - fails.len();
    for f in &fails {
        println!("[leashshot] FAIL  {f}");
    }
    println!("[leashshot] witnesses observed: {passed} / {total}");
    if fails.is_empty() {
        println!("[leashshot] PASS — {total} witnesses");
        Ok(())
    } else {
        Err(format!("leashshot: {} of {total} witnesses failed", fails.len()))
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

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

fn span(v: &[u32]) -> u32 {
    match (v.iter().min(), v.iter().max()) {
        (Some(&a), Some(&b)) => b - a,
        _ => 0,
    }
}
