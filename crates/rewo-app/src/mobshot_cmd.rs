//! `rewo mobshot` — serverless mob-model verification (headless, no MC
//! server needed).
//!
//! Two modes:
//! - **Contact sheet** (default): render every available mob with its real
//!   texture into one PNG — the eyeball artifact.
//! - **`--check`**: the facelabel gate (REWO_MOB_REDO_HANDOFF §6). Every
//!   mob texture is replaced by per-face solid colors
//!   (`REWO_MOB_DEBUG_TEX`), each mob renders from front/left/top, and the
//!   dominant rendered color must match the dominant face *predicted from
//!   the model geometry itself* (projected camera-facing area by face
//!   label). A scrambled UV unwrap makes the rendered color diverge from
//!   the geometric prediction — the exact bug class the old hand-rolled
//!   unwrap shipped. Runs twice per view (with and without the mob) and
//!   only classifies pixels that differ, so sky/fog/overlay can't pollute
//!   the counts.
//!
//! In-rect rotations/flips are invisible to solid colors — those are pinned
//! by the exact per-vertex UV unit tests in `rewo_gpu::mobs`.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths};
use rewo_gpu::entities::EntityDraw;
use rewo_gpu::mobs::{EntityModelKind, Facing};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const BG: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
const FACES: [Facing; 6] = [
    Facing::Down,
    Facing::Up,
    Facing::West,
    Facing::North,
    Facing::East,
    Facing::South,
];

#[derive(ClapArgs)]
pub struct MobshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Contact-sheet output (default mode) or, with --check, unused.
    #[arg(long, default_value = "rewo-mobs.png")]
    out: PathBuf,
    /// Facelabel verification: assert texture-face correspondence for every
    /// mob from 3 angles; nonzero exit on any mismatch.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// With --check: also dump each per-view debug render here.
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Sheet mode: render only these mobs (comma-separated kind names) in a
    /// closer 3/4 view — the detail-inspection artifact.
    #[arg(long)]
    only: Option<String>,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: MobshotArgs) -> Result<(), String> {
    if args.check {
        // Must be set before the entity pass bakes its atlas.
        std::env::set_var("REWO_MOB_DEBUG_TEX", "1");
    }
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;

    let result = if args.check {
        run_check(&mut gpu, &baked, &args)
    } else {
        run_sheet(&mut gpu, &baked, &args)
    };
    result
}

/// Hidden overlay: the offscreen render always draws the frame-time chart,
/// so park it far outside the viewport.
fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn neutral_draw(kind: EntityModelKind) -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0; 3],
        width: 1.0,
        height: 2.0,
        color: [1.0, 0.0, 1.0],
        name: None,
        kind,
        yaw: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
    }
}

// ---------------------------------------------------------------------------
// --check: the facelabel gate
// ---------------------------------------------------------------------------

fn run_check(gpu: &mut Gpu, baked: &assets::BakedAssets, args: &MobshotArgs) -> Result<(), String> {
    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(gpu, crate::live_cmd::font_data(baked), crate::live_cmd::entity_textures(baked))?;
    let kinds = wr.entity_pass().expect("entity pass").available_kinds();
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }

    // (label, view direction the camera looks along, up vector).
    let views: [(&str, Vec3, Vec3); 3] = [
        ("front", Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        ("left", Vec3::new(-1.0, 0.0, 0.0), Vec3::Y),
        ("top", Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, -1.0)),
    ];
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for kind in kinds {
        let quads = wr
            .entity_pass()
            .expect("entity pass")
            .neutral_quads(kind)
            .expect("kind was listed available");
        // World-space bbox of the neutral pose (entity at origin, yaw 0).
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (pos, _, _) in &quads {
            for p in pos {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
        let center = Vec3::new(
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        );
        let radius = (0..3).map(|i| hi[i] - lo[i]).fold(0f32, f32::max) * 0.5;
        let dist = radius * 3.0 + 1.0;

        for (view_name, dir, up) in views {
            let eye = center - dir * dist;
            let predicted = predict_counts(&quads, eye, dir, up);
            let (pi, &pmax) = predicted
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| **c)
                .expect("6 faces");
            if pmax == 0 {
                failures.push(format!("{kind:?}/{view_name}: no camera-facing quads"));
                continue;
            }
            let expected = FACES[pi];
            let view = Mat4::look_to_rh(eye, dir, up);
            let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
                55f32.to_radians(),
                1.0,
                0.05,
            ));
            let vp = (proj * view).to_cols_array_2d();
            wr.set_camera(eye.to_array());
            let right = dir.cross(Vec3::Y).normalize_or_zero().to_array();
            let upv = up.to_array();

            // Background-only pass, then the mob; classify only changed
            // pixels so sky/fog/chart can't leak into the counts.
            wr.set_entities(&[], right, upv);
            off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
            let bg = off.read_rgba(gpu)?;
            wr.set_entities(&[neutral_draw(kind)], right, upv);
            off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
            let img = off.read_rgba(gpu)?;
            if let Some(dir) = &args.out_dir {
                save_png(&img, w, h, &dir.join(format!("{}-{view_name}.png", kind_name(kind))))?;
            }

            let mut counts = [0usize; 6];
            let mut changed = 0usize;
            for i in (0..img.len()).step_by(8) {
                if img[i..i + 3] != bg[i..i + 3] {
                    changed += 1;
                    if let Some(f) = classify(&img[i..i + 4]) {
                        counts[face_index(f)] += 1;
                    }
                }
            }
            let (di, &dc) = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| **c)
                .expect("6 faces");
            checked += 1;
            // Strict when the prediction is unambiguous; a near-tie (the
            // rendered dominant predicted within 80% of the predicted max,
            // e.g. the enderman's half-mirrored left profile) passes too —
            // a scrambled unwrap still scores ~0 and fails hard.
            let near_tie = predicted[di] * 5 >= pmax * 4;
            if changed < 400 {
                failures.push(format!(
                    "{kind:?}/{view_name}: mob barely visible ({changed} px changed)"
                ));
            } else if dc == 0 || (FACES[di] != expected && !near_tie) {
                failures.push(format!(
                    "{kind:?}/{view_name}: expected {expected:?} (predicted {predicted:?}), rendered dominant {:?} (counts {counts:?})",
                    FACES[di]
                ));
            }
        }
    }

    wr.destroy(gpu);
    off.destroy(gpu);
    if failures.is_empty() {
        println!("[mobshot] CHECK OK — {checked} mob-views match their geometric face labels");
        Ok(())
    } else {
        for f in &failures {
            println!("[mobshot] FAIL {f}");
        }
        Err(format!("mobshot check: {} of {checked} views failed", failures.len()))
    }
}

/// Per-face-label visibility counts by ray-casting the **same perspective
/// camera** the render uses (same eye, direction, fov), keeping each ray's
/// nearest hit — a tiny reference renderer of labels. Occlusion and
/// projection match the real render exactly (a chicken from above shows its
/// rotated body's South rect, a villager its hat rim, a zombie's high hat
/// beats its outstretched arms). Purely geometric — independent of the UV
/// path under test.
fn predict_counts(
    quads: &[([[f32; 3]; 4], Facing, [f32; 3])],
    eye: Vec3,
    dir: Vec3,
    up: Vec3,
) -> [usize; 6] {
    let right = dir.cross(up).normalize();
    let real_up = right.cross(dir);
    let half = (55f32.to_radians() / 2.0).tan();
    const N: usize = 128;
    let mut counts = [0usize; 6];
    for iy in 0..N {
        for ix in 0..N {
            let sx = (2.0 * (ix as f32 + 0.5) / N as f32 - 1.0) * half;
            let sy = (1.0 - 2.0 * (iy as f32 + 0.5) / N as f32) * half;
            let rd = (dir + right * sx + real_up * sy).normalize();
            let mut best = (f32::MAX, None);
            for (pos, facing, _) in quads {
                let p: [Vec3; 4] = [
                    Vec3::from_array(pos[0]),
                    Vec3::from_array(pos[1]),
                    Vec3::from_array(pos[2]),
                    Vec3::from_array(pos[3]),
                ];
                for tri in [[0, 1, 2], [0, 2, 3]] {
                    if let Some(t) = ray_tri(eye, rd, p[tri[0]], p[tri[1]], p[tri[2]]) {
                        if t < best.0 {
                            best = (t, Some(*facing));
                        }
                    }
                }
            }
            if let Some(f) = best.1 {
                counts[face_index(f)] += 1;
            }
        }
    }
    counts
}

/// Möller–Trumbore, both-sided (mirrored cubes flip winding).
fn ray_tri(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let uu = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&uu) {
        return None;
    }
    let q = s.cross(e1);
    let vv = dir.dot(q) * inv;
    if vv < 0.0 || uu + vv > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Map a rendered debug texel back to its face label by chroma pattern —
/// robust under the per-face shade multiply and the sRGB round-trip.
fn classify(px: &[u8]) -> Option<Facing> {
    let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
    let max = r.max(g).max(b);
    if max < 40 {
        return None;
    }
    let bit = |c: i32| c * 2 > max;
    match (bit(r), bit(g), bit(b)) {
        (true, false, false) => Some(Facing::North),
        (true, true, false) => Some(Facing::South),
        (false, true, false) => Some(Facing::West),
        (false, false, true) => Some(Facing::East),
        (true, false, true) => Some(Facing::Down),
        (false, true, true) => Some(Facing::Up),
        _ => None,
    }
}

fn face_index(f: Facing) -> usize {
    FACES.iter().position(|x| *x == f).unwrap()
}

fn kind_name(k: EntityModelKind) -> &'static str {
    k.name()
}

// ---------------------------------------------------------------------------
// Contact sheet
// ---------------------------------------------------------------------------

fn run_sheet(gpu: &mut Gpu, baked: &assets::BakedAssets, args: &MobshotArgs) -> Result<(), String> {
    let (w, h) = (2560u32, 1440u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(gpu, crate::live_cmd::font_data(baked), crate::live_cmd::entity_textures(baked))?;
    let pass = wr.entity_pass().expect("entity pass");
    let mut kinds = pass.available_kinds();
    if let Some(only) = &args.only {
        let wanted: Vec<&str> = only.split(',').map(str::trim).collect();
        kinds.retain(|k| wanted.contains(&k.name()));
        if kinds.is_empty() {
            return Err(format!("--only matched no mobs: {only}"));
        }
    }
    println!(
        "[mobshot] {} models: {}",
        kinds.len(),
        kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
    );
    // Short mobs in the front rows, giants (ghast, golem, camel) in the
    // back — nothing hides behind something taller.
    let height_of = |k: &rewo_gpu::mobs::EntityModelKind| -> f32 {
        pass.neutral_quads(*k)
            .map(|qs| {
                qs.iter()
                    .flat_map(|(pos, _, _)| pos.iter().map(|p| p[1]))
                    .fold(0.0f32, f32::max)
            })
            .unwrap_or(2.0)
    };
    kinds.sort_by(|a, b| height_of(a).total_cmp(&height_of(b)));

    // Grid facing the camera (mobs face +Z at yaw 0; camera sits south).
    // Rows step away from the camera; odd rows stagger half a column so
    // nobody hides directly behind the mob in front.
    let cols = 10usize;
    let (sx, sz) = (3.8f32, 6.5f32);
    let rows = kinds.len().div_ceil(cols);
    let draws: Vec<EntityDraw<'_>> = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let mut d = neutral_draw(*k);
            let (row, col) = (i / cols, i % cols);
            let row_n = ((kinds.len() - row * cols).min(cols)) as f32;
            let stagger = if row % 2 == 1 { sx * 0.5 } else { 0.0 };
            d.pos = [
                (col as f32 - (row_n - 1.0) / 2.0) * sx + stagger,
                0.0,
                -(row as f32) * sz,
            ];
            d
        })
        .collect();

    let max_h = kinds.iter().map(|k| height_of(k)).fold(2.0f32, f32::max);
    let center = Vec3::new(0.0, (max_h * 0.45).max(1.0), -((rows as f32 - 1.0) * sz) / 2.0);
    let eye = if args.only.is_some() {
        // Closeup: 3/4 view fitted to the row width + tallest mob.
        let span = (kinds.len().min(cols) as f32) * sx;
        let dist = (span * 0.55).max(max_h * 1.9) + 3.0;
        center + Vec3::new(dist * 0.4, dist * 0.35, dist)
    } else {
        center + Vec3::new(0.0, 11.0, (rows as f32) * sz * 0.6 + 12.0)
    };
    let dir = (center - eye).normalize();
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        55f32.to_radians(),
        w as f32 / h as f32,
        0.05,
    ));
    let vp = (proj * view).to_cols_array_2d();
    let right = dir.cross(Vec3::Y).normalize_or_zero().to_array();
    let up = right_up(dir);
    wr.set_entities(&draws, right, up);

    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    for _ in 0..3 {
        off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    }
    off.save_png(gpu, &args.out)?;
    println!("[mobshot] sheet: {} mobs, wrote {}", kinds.len(), args.out.display());
    wr.destroy(gpu);
    off.destroy(gpu);
    Ok(())
}

fn right_up(dir: Vec3) -> [f32; 3] {
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    right.cross(dir).normalize_or_zero().to_array()
}

fn save_png(rgba: &[u8], w: u32, h: u32, path: &PathBuf) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wtr| wtr.write_image_data(rgba))
        .map_err(|e| format!("png {path:?}: {e}"))?;
    Ok(())
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}
