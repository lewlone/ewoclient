//! `rewo bench` — M6 deterministic render benchmark: the metric that governs
//! merges (REWO_PLAN.md §3). Loads a replay world, meshes it into the
//! GPU-driven renderer, and renders N frames from a **deterministic camera
//! orbit** (same scene + same path every run → trustworthy A/B), capturing
//! per-frame GPU timestamps. Reports the full frame-consistency suite:
//! avg / p50 / p99 / p99.9 / **1% low / 0.1% low** / max, plus a histogram.
//!
//! Headless + deterministic — a machine runs it and compares runs; a change
//! that raises average FPS but worsens the 0.1% low is a regression.

use std::path::PathBuf;
use std::time::Instant;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths, GameData};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_net::{ids::Ids, record};
use rewo_world::World;

use crate::stats::{OverlayRing, StatsAccum};

const CLEAR_SKY: [f32; 4] = [0.184, 0.380, 1.0, 1.0];

#[derive(ClapArgs)]
pub struct BenchArgs {
    /// Replay file to benchmark against (a recorded packet session).
    #[arg(long)]
    replay: PathBuf,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Frames to render.
    #[arg(long, default_value_t = 2000)]
    frames: u32,
    /// Warmup frames excluded from the stats (first-frame / cache effects).
    #[arg(long, default_value_t = 100)]
    warmup: u32,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: BenchArgs) -> Result<(), String> {
    // -- world + assets ----------------------------------------------------
    let data = GameData::load_for_version(&args.version)?;
    let ids = Ids::resolve(&data.packets)?;
    let (world, chunks) = record::replay(&args.replay, &data, &ids)?;
    if world.loaded_columns() == 0 {
        return Err("bench: replay has no chunks".into());
    }
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    log::info!("bench: replayed {chunks} chunks, baking done");

    // -- mesh + upload -----------------------------------------------------
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut wr = WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    let mut total_verts = 0usize;
    for (cx, cz) in world.column_coords() {
        if let Some(mesh) = rewo_mesh::mesh_column(&world, &baked.render, &baked.models, cx, cz) {
            total_verts += mesh.vertices.len();
            wr.upload_column(
                &mut gpu,
                cx,
                cz,
                bytemuck::cast_slice(&mesh.vertices),
                &mesh.indices,
                bytemuck::cast_slice(&mesh.tvertices),
                &mesh.tindices,
                mesh.y_min,
                mesh.y_max,
            )?;
        }
    }
    gpu.wait_idle();

    let center = world_center(&world);
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };

    // -- render loop -------------------------------------------------------
    let mut gpu_ms = StatsAccum::default();
    let mut wall_ms = StatsAccum::default();
    let mut last_cull: u32 = 0;
    let start = Instant::now();
    let total = args.frames + args.warmup;
    for i in 0..total {
        let (vp, eye) = orbit(center, i, total);
        wr.set_camera(eye.to_array());
        let t0 = Instant::now();
        off.render(&gpu, Some((&mut wr, vp)), &draw, CLEAR_SKY)?;
        let wall = t0.elapsed().as_secs_f32() * 1000.0;
        if i >= args.warmup {
            if let Some(g) = off.last_gpu_ms {
                gpu_ms.push(g);
            }
            wall_ms.push(wall);
        }
        if i == total - 1 {
            last_cull = wr.read_draw_count(&mut gpu);
        }
    }
    let elapsed = start.elapsed().as_secs_f32();

    // -- report ------------------------------------------------------------
    println!("[rewo-m6] bench: {} chunks, {} verts", world.loaded_columns(), total_verts);
    println!(
        "[rewo-m6] {} frames ({} warmup) in {:.2}s — orbit camera, GPU cull {} of {} last frame",
        gpu_ms.len(),
        args.warmup,
        elapsed,
        last_cull,
        wr.column_count(),
    );
    report("GPU frame time (render cost, from timestamps)", &gpu_ms);
    report("wall frame time (serialized offscreen — CPU+GPU)", &wall_ms);
    println!("[rewo-m6] GPU frame-time histogram:");
    print!("{}", gpu_ms.histogram(16, gpu_ms.low_mean(0.001).max(0.1) * 1.5));
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

fn report(label: &str, s: &StatsAccum) {
    if s.is_empty() {
        return;
    }
    println!("[rewo-m6] {label}:");
    println!(
        "[rewo-m6]   avg {:.3}  p50 {:.3}  p99 {:.3}  p99.9 {:.3}  max {:.3} ms",
        s.average(),
        s.percentile(0.50),
        s.percentile(0.99),
        s.percentile(0.999),
        s.percentile(1.0),
    );
    println!(
        "[rewo-m6]   1% low {:.3} ms   0.1% low {:.3} ms   (mean of the slowest frames)",
        s.low_mean(0.01),
        s.low_mean(0.001),
    );
}

/// Deterministic orbit: 3 full turns over the run, radius scaled to the
/// loaded area, hovering above the surface and looking at the center.
fn orbit(center: Vec3, frame: u32, total: u32) -> ([[f32; 4]; 4], Vec3) {
    let t = frame as f32 / total.max(1) as f32;
    let angle = t * std::f32::consts::TAU * 3.0;
    let radius = 90.0;
    let eye = center + Vec3::new(angle.cos() * radius, 40.0, angle.sin() * radius);
    let view = Mat4::look_at_rh(eye, center, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        1280.0 / 720.0,
        0.05,
    ));
    ((proj * view).to_cols_array_2d(), eye)
}

fn world_center(world: &World) -> Vec3 {
    let coords = world.column_coords();
    let n = coords.len().max(1) as f32;
    let (sx, sz) = coords.iter().fold((0.0f32, 0.0f32), |(ax, az), (cx, cz)| {
        (ax + *cx as f32 * 16.0 + 8.0, az + *cz as f32 * 16.0 + 8.0)
    });
    Vec3::new(sx / n, world.shape.min_y as f32 + 4.0, sz / n)
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}
