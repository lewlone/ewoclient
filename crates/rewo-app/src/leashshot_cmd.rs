//! `rewo leashshot --check` — the serverless gate for the leash rope (M170).
//!
//! It renders `leash::build_ribbon`'s output through the production
//! `WorldRenderer::draw_leash` pass into an offscreen target and reads the
//! pixels back, so it grades the whole GPU path — the pipeline, the vertex
//! format, the depth/blend config — not just the geometry (which the module's
//! own unit tests pin). The CPU gather (`collect_leashes`) is live-only and is
//! graded by `live --render-check`'s r60.
//!
//! Six witnesses, each with a control that must find nothing or the opposite:
//! an empty frame draws no rope; a level rope draws brown pixels (g1a) that form
//! a thin horizontal line (g1b); a slack rope sags below a taut one; the
//! alternating segments carry two brown shades; and the light fades along a rope
//! lit at one end. **Fail-closed** on [`EXPECTED_WITNESSES`]: every witness
//! records unconditionally (a fail is a recorded fail, never a skipped branch),
//! and a count that is not exactly the pinned one fails.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};

use rewo_data::{assets, DataPaths};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

/// `g0`, `g1a`, `g1b`, `g2`, `g3`, `g4`.
const EXPECTED_WITNESSES: usize = 6;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;
const LIT: (u8, u8) = (15, 15);
const DARK: (u8, u8) = (0, 0);

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
        return Err(
            "leashshot check: Vulkan validation requested but not active — \
            install the Vulkan SDK, or pass --no-validation"
                .into(),
        );
    }
    run_check(&mut gpu, &baked, &args)
}

/// Camera looking straight down +Z from `(0, 0, -dist)`, framing a rope drawn
/// around the world origin — the exact `look_to_rh` + `perspective_reverse_z`
/// convention the live path uses (`eye_view` in `live_cmd`).
fn view_proj(dist: f32) -> [[f32; 4]; 4] {
    let eye = Vec3::new(0.0, 0.0, -dist);
    let view = crate::view_math::look_to_rh(eye, Vec3::Z, Vec3::Y);
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

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[leashshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
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
    // Daylight lightmap state: the witnesses reason about BLOCK levels, which
    // sample() resolves against the same curve the world pass uses.
    let lm = rewo_world::lightmap::LightmapState::default();
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    // A closure that renders `verts` and reads the frame back.
    let frame = |gpu: &mut Gpu,
                 off: &mut Offscreen,
                 verts: &[rewo_gpu::leash::LeashVertex],
                 dist: f32|
     -> Result<Vec<u8>, String> {
        let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
        wr.set_camera([0.0, 0.0, -dist]);
        wr.set_leash(verts);
        let rendered = off
            .render(gpu, Some((&mut wr, view_proj(dist))), &draw, CLEAR)
            .and_then(|()| off.read_rgba(gpu));
        // A renderer per frame, so it must be torn down per frame — leaking it
        // left 512 child objects alive at vkDestroyDevice.
        wr.destroy(gpu);
        rendered
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
    c.record(
        "g0",
        n_empty <= 20,
        format!("empty frame has {n_empty} rope pixels (want ~0)"),
    );

    // -- g1: a level rope draws a thin brown line -------------------------
    let level = build_ribbon(
        [-2.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        false,
        LIT,
        LIT,
        &|b, s| rewo_world::lightmap::sample(b, s, &lm),
    );
    let f1 = frame(gpu, &mut off, &level, 5.0)?;
    dump(&mut off, gpu, "level");
    let p1 = rope_pixels(&f1);
    c.record(
        "g1a",
        p1.len() >= 200,
        format!(
            "level rope has {} rope pixels (want a line, >= 200)",
            p1.len()
        ),
    );
    // A horizontal rope: much wider than it is tall. With no pixels both spans
    // are 0 and the witness fails (0 < 0 is false).
    let xs: Vec<u32> = p1.iter().map(|&(x, _, _)| x).collect();
    let ys: Vec<u32> = p1.iter().map(|&(_, y, _)| y).collect();
    let (xspan, yspan) = (span(&xs), span(&ys));
    c.record(
        "g1b",
        xspan >= 1 && xspan >= yspan * 3,
        format!("x span {xspan}, y span {yspan} (want x >= 3 * y)"),
    );

    // -- g2: a slack rope sags below a taut one ---------------------------
    // A rope to a point below: slack curves under the straight interpolation.
    let start = [-2.0, 1.5, 0.0];
    let end = [2.0, -1.5, 0.0];
    let slack = build_ribbon(start, end, true, LIT, LIT, &|b, s| {
        rewo_world::lightmap::sample(b, s, &lm)
    });
    let taut = build_ribbon(start, end, false, LIT, LIT, &|b, s| {
        rewo_world::lightmap::sample(b, s, &lm)
    });
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
    let (sl, tl) = (mid_low(&rope_pixels(&fs)), mid_low(&rope_pixels(&ft)));
    c.record(
        "g2",
        matches!((sl, tl), (Some(sl), Some(tl)) if sl > tl + 2),
        format!("slack low y {sl:?}, taut low y {tl:?} (slack must sag > 2 px lower)"),
    );

    // -- g3: alternating segments carry two brown shades ------------------
    // On the level rope, the reds cluster into a bright and a dimmed value
    // (the 0.7 colorModifier). Assert the spread of distinct reds spans it.
    let reds: Vec<u8> = p1.iter().map(|&(_, _, r)| r).collect();
    // Bright/dim ratio is 1/0.7 ~= 1.43; even after sRGB curvature the two
    // shades differ by well over a threshold. Guard against a flat rope. No
    // reds at all is a failure, not a skip.
    let red_range = reds.iter().min().zip(reds.iter().max());
    c.record(
        "g3",
        matches!(red_range, Some((&lo, &hi)) if hi >= lo.saturating_add(8)),
        format!("reds span {red_range:?} (want a spread of >= 8)"),
    );

    // -- g4: light fades along a rope lit at one end ----------------------
    let faded = build_ribbon(
        [-2.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        false,
        LIT,
        DARK,
        &|b, s| rewo_world::lightmap::sample(b, s, &lm),
    );
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
    let avg = |v: &[u8]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().map(|&r| r as f32).sum::<f32>() / v.len() as f32
        }
    };
    let (la, ra) = (avg(&left), avg(&right));
    c.record(
        "g4",
        (la - ra).abs() >= 10.0,
        format!("left avg red {la:.1}, right avg red {ra:.1} (want a gap >= 10)"),
    );

    off.destroy(gpu);

    println!(
        "[leashshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "leashshot: {} witnesses failed: {}",
            c.failures.len(),
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "leashshot: witness count {} != expected {EXPECTED_WITNESSES} — a named property              was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[leashshot] PASS — {} witnesses", c.witnessed);
    Ok(())
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
