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
    /// Repeat **only** `mesh_column` this many times to sample meshing cost
    /// (minimum 1). Asset bake and replay happen before; GPU init, upload and
    /// render all happen after, so nothing GPU-side is live during the
    /// measurement. Exactly one mesh set (the last run's) is preserved for
    /// upload/render. Use >= 4 for an explicit M15 sample.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    mesh_runs: u32,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

/// Bytes one vertex occupies on upload — the **actual** Rust layout, never a
/// literal, so an ABI change is reflected rather than assumed.
const VERTEX_BYTES: u64 = std::mem::size_of::<rewo_mesh::MeshVertex>() as u64;
/// Indices are `u32` (see `upload_column`).
const INDEX_BYTES: u64 = std::mem::size_of::<u32>() as u64;

/// Deterministic geometry census for one meshed world snapshot.
///
/// The counts are a pure function of the replay + bake, so they are directly
/// comparable across runs and across a future ABI change. The *timings* reported
/// beside them are wall clock and noisy — only these counts are deterministic.
///
/// Opaque and translucent are tracked separately **and** summed: the GPU vertex
/// and index arenas are shared pools (`rewo_gpu::world::MAX_VERTS` /
/// `MAX_INDICES`), so any utilization figure that omits translucent geometry
/// understates real pressure.
///
/// The greedy cube-face census (M15) rides along. Each `ColumnMesh` counter is
/// a `u32` **per column**; a replay of a few hundred columns can sum past
/// `u32::MAX`, so every one widens to `u64` at the fold — a wrapped total would
/// under-report visible faces and read as *better* merging rather than as
/// corruption.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct GeomSummary {
    /// Columns that produced geometry and therefore consume an arena slot.
    /// **Not** the number of coordinates the mesher walked — a coordinate that
    /// meshes to nothing (all-air column) still costs time but is absent here,
    /// so this must never be used to normalize timing (see [`ms_per_column`]).
    pub uploaded_columns: u64,
    pub opaque_verts: u64,
    pub opaque_indices: u64,
    pub trans_verts: u64,
    pub trans_indices: u64,
    /// Largest single column by **combined** opaque+translucent vertices/bytes.
    pub max_column_verts: u64,
    pub max_column_bytes: u64,
    /// Columns folded into the greedy census below. Tracked separately from
    /// [`Self::uploaded_columns`] purely as a guard: the two folds are distinct
    /// calls, so a column that reaches one and not the other would otherwise
    /// silently shrink the face totals. `check_greedy_invariants` rejects the
    /// mismatch.
    pub greedy_columns: u64,
    /// Every visible (un-culled) cube unit face the scan saw, summed. Models and
    /// fluids are not counted — only the cube path can merge.
    pub visible_cube_faces: u64,
    /// Of those, the ones eligible to enter a greedy mask.
    pub greedy_candidate_faces: u64,
    /// Rectangles actually emitted from the candidates. The candidates→quads
    /// ratio is the compression the pass bought; see
    /// [`Self::candidate_quad_reduction_percent`].
    pub greedy_quads: u64,
    /// Visible cube faces that did not merge and were emitted as legacy unit
    /// quads. `visible_cube_faces == greedy_candidate_faces + unit_fallback_faces`.
    pub unit_fallback_faces: u64,
}

impl GeomSummary {
    /// Fold one column's four counts in. Deliberately takes raw counts rather
    /// than a `ColumnMesh` so the arithmetic is directly testable.
    pub fn add_column(&mut self, ov: u64, oi: u64, tv: u64, ti: u64) {
        self.uploaded_columns += 1;
        self.opaque_verts += ov;
        self.opaque_indices += oi;
        self.trans_verts += tv;
        self.trans_indices += ti;
        let combined_verts = ov + tv;
        let combined_bytes = combined_verts * VERTEX_BYTES + (oi + ti) * INDEX_BYTES;
        self.max_column_verts = self.max_column_verts.max(combined_verts);
        self.max_column_bytes = self.max_column_bytes.max(combined_bytes);
    }

    /// Fold one column's greedy cube-face census in. Kept separate from
    /// [`Self::add_column`] for the same reason that one takes raw counts: the
    /// arithmetic stays directly testable without a `ColumnMesh`. Every
    /// `u32` widens here, so the totals cannot wrap across a large replay.
    pub fn add_greedy(&mut self, visible: u64, candidates: u64, quads: u64, fallback: u64) {
        self.greedy_columns += 1;
        self.visible_cube_faces += visible;
        self.greedy_candidate_faces += candidates;
        self.greedy_quads += quads;
        self.unit_fallback_faces += fallback;
    }

    /// Compression the greedy pass bought, as a percentage: `100 * (1 -
    /// quads/candidates)`.
    ///
    /// The denominator is **candidates**, not visible faces — unit fallbacks
    /// were never offered to the merge, so charging them against it would
    /// understate the pass by exactly the fallback fraction. Returns 0.0 when
    /// nothing was a candidate, rather than a NaN from 0/0.
    pub fn candidate_quad_reduction_percent(&self) -> f64 {
        if self.greedy_candidate_faces == 0 {
            return 0.0;
        }
        100.0 * (1.0 - self.greedy_quads as f64 / self.greedy_candidate_faces as f64)
    }

    /// Reject a census that cannot be true, so the reported M15 numbers are
    /// never a quiet fiction. Mirrors the per-column invariants `rewo-mesh`
    /// asserts, which survive summation:
    /// - every visible cube face is either a candidate or a fallback,
    /// - a rectangle consumes at least one candidate, so `quads <= candidates`,
    /// - and both folds saw the same columns.
    pub fn check_greedy_invariants(&self) -> Result<(), String> {
        if self.greedy_columns != self.uploaded_columns {
            return Err(format!(
                "greedy census covers {} columns but {} were uploaded — a column was omitted from the face census",
                self.greedy_columns, self.uploaded_columns
            ));
        }
        let split = self.greedy_candidate_faces + self.unit_fallback_faces;
        if self.visible_cube_faces != split {
            return Err(format!(
                "greedy census: {} visible cube faces != {} candidates + {} fallbacks ({split})",
                self.visible_cube_faces, self.greedy_candidate_faces, self.unit_fallback_faces
            ));
        }
        if self.greedy_quads > self.greedy_candidate_faces {
            return Err(format!(
                "greedy census: {} rectangles from only {} candidates — a rectangle consumes at least one candidate",
                self.greedy_quads, self.greedy_candidate_faces
            ));
        }
        Ok(())
    }

    pub fn from_meshes(meshes: &[rewo_mesh::ColumnMesh]) -> Self {
        let mut s = Self::default();
        for m in meshes {
            s.add_column(
                m.vertices.len() as u64,
                m.indices.len() as u64,
                m.tvertices.len() as u64,
                m.tindices.len() as u64,
            );
            s.add_greedy(
                m.visible_cube_faces as u64,
                m.greedy_candidate_faces as u64,
                m.greedy_quads as u64,
                m.unit_fallback_faces as u64,
            );
        }
        s
    }

    pub fn total_verts(&self) -> u64 {
        self.opaque_verts + self.trans_verts
    }

    pub fn total_indices(&self) -> u64 {
        self.opaque_indices + self.trans_indices
    }

    /// Total bytes uploaded into the shared arenas.
    pub fn total_bytes(&self) -> u64 {
        self.total_verts() * VERTEX_BYTES + self.total_indices() * INDEX_BYTES
    }

    /// Fraction (0..1) of each shared pool consumed by opaque+translucent.
    pub fn vert_utilization(&self) -> f64 {
        self.total_verts() as f64 / rewo_gpu::world::MAX_VERTS as f64
    }

    pub fn index_utilization(&self) -> f64 {
        self.total_indices() as f64 / rewo_gpu::world::MAX_INDICES as f64
    }

    /// Column-slot pressure. Correctly based on **uploaded** columns: an empty
    /// column never reaches `upload_column`, so it occupies no metadata slot.
    pub fn column_utilization(&self) -> f64 {
        self.uploaded_columns as f64 / rewo_gpu::world::MAX_COLUMNS as f64
    }
}

/// Normalize a mesh-run duration by the number of coordinates the timed loop
/// actually **attempted**, not by the non-empty results.
///
/// The loop walks every loaded coordinate and pays `mesh_column` for each; a
/// coordinate that yields no geometry still costs time but produces no
/// `ColumnMesh`. Dividing by the non-empty count would therefore inflate
/// ms/column by exactly the empty fraction — invisible on a replay where every
/// loaded column happens to be non-empty.
fn ms_per_column(total_ms: f32, attempted_columns: usize) -> f32 {
    total_ms / attempted_columns.max(1) as f32
}

/// min / median / max of a small sample (median averages the middle pair when
/// the count is even).
fn min_med_max(v: &[f32]) -> (f32, f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let n = s.len();
    let median = if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    };
    (s[0], median, s[n - 1])
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

    // A silent disagreement here would corrupt every upload offset, so fail
    // fast rather than benchmark a broken layout.
    if VERTEX_BYTES != rewo_gpu::world::VERTEX_STRIDE {
        return Err(format!(
            "vertex stride mismatch: size_of::<MeshVertex>() = {VERTEX_BYTES} but \
             rewo_gpu::world::VERTEX_STRIDE = {}",
            rewo_gpu::world::VERTEX_STRIDE
        ));
    }

    // -- mesh (measured) ---------------------------------------------------
    // `--mesh-runs` repeats ONLY `mesh_column`. The replay and asset bake are
    // already done; **no Vulkan device exists yet** — GPU init is deliberately
    // deferred past this loop so no driver thread runs alongside the CPU
    // measurement. Earlier runs' meshes drop at the end of their iteration,
    // after the timer stops.
    let coords = world.column_coords();
    let runs = args.mesh_runs; // clap enforces >= 1
    let mut mesh_ms: Vec<f32> = Vec::with_capacity(runs as usize);
    let mut kept: Vec<rewo_mesh::ColumnMesh> = Vec::new();
    for r in 0..runs {
        let t0 = Instant::now();
        let meshes: Vec<rewo_mesh::ColumnMesh> = coords
            .iter()
            .filter_map(|(cx, cz)| {
                rewo_mesh::mesh_column(&world, &baked.render, &baked.models, &baked.fluid, *cx, *cz)
            })
            .collect();
        mesh_ms.push(t0.elapsed().as_secs_f32() * 1000.0);
        if r + 1 == runs {
            kept = meshes; // exactly one set survives, for upload/render
        }
    }
    let geom = GeomSummary::from_meshes(&kept);
    let attempted_columns = coords.len();
    // Same precedent as the vertex-stride check above: an inconsistent census
    // makes every M15 greedy figure a fiction, so fail loud before spending a
    // GPU init and 2000 frames producing numbers nobody should trust.
    geom.check_greedy_invariants()?;

    // -- GPU init + upload (after all measurement) --------------------------
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut wr = WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;

    for m in &kept {
        wr.upload_column(
            &mut gpu,
            m.cx,
            m.cz,
            bytemuck::cast_slice(&m.vertices),
            &m.indices,
            bytemuck::cast_slice(&m.tvertices),
            &m.tindices,
            m.y_min,
            m.y_max,
        )?;
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

    // -- geometry census (deterministic) + mesh timing (noisy) --------------
    println!(
        "[rewo-m15] geometry: {} loaded columns attempted, {} non-empty/uploaded   opaque {} verts / {} idx   translucent {} verts / {} idx",
        attempted_columns,
        geom.uploaded_columns,
        geom.opaque_verts,
        geom.opaque_indices,
        geom.trans_verts,
        geom.trans_indices
    );
    println!(
        "[rewo-m15]   combined {} verts / {} idx   upload {:.2} MiB   (vertex {VERTEX_BYTES} B = size_of MeshVertex, index {INDEX_BYTES} B)",
        geom.total_verts(),
        geom.total_indices(),
        geom.total_bytes() as f64 / (1024.0 * 1024.0),
    );
    println!(
        "[rewo-m15]   largest column: {} verts, {:.1} KiB (combined opaque+translucent)",
        geom.max_column_verts,
        geom.max_column_bytes as f64 / 1024.0,
    );
    println!(
        "[rewo-m15]   shared-arena utilization (opaque+translucent): vertex {:.3}% of {}, index {:.3}% of {}, columns {:.3}% of {}",
        geom.vert_utilization() * 100.0,
        rewo_gpu::world::MAX_VERTS,
        geom.index_utilization() * 100.0,
        rewo_gpu::world::MAX_INDICES,
        geom.column_utilization() * 100.0,
        rewo_gpu::world::MAX_COLUMNS,
    );
    // Deterministic like the counts above (a pure function of replay + bake),
    // and invariant-checked at the census point.
    println!(
        "[rewo-m15] greedy cubes: {} visible unit faces = {} candidates + {} unit fallbacks; {} rectangles emitted from candidates ({:.1}% candidate quad reduction)",
        geom.visible_cube_faces,
        geom.greedy_candidate_faces,
        geom.unit_fallback_faces,
        geom.greedy_quads,
        geom.candidate_quad_reduction_percent(),
    );
    let (mesh_min, mesh_med, mesh_max) = min_med_max(&mesh_ms);
    let per_col = ms_per_column(mesh_med, attempted_columns);
    println!(
        "[rewo-m15] mesh_column over {} SERIAL single-threaded run(s) on this thread — NOT the production rayon MeshPool path, so this is per-column cost, not production meshing throughput; no GPU device live — WALL CLOCK, noisy, NOT deterministic (only the counts above are): min {:.1} / median {:.1} / max {:.1} ms, {:.3} ms per ATTEMPTED column at median ({} attempted)",
        runs, mesh_min, mesh_med, mesh_max, per_col, attempted_columns,
    );
    println!(
        "[rewo-m15]   per-run ms: {}",
        mesh_ms
            .iter()
            .map(|m| format!("{m:.1}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // -- report ------------------------------------------------------------
    println!("[rewo-m6] bench: {} chunks", world.loaded_columns());
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
    print!(
        "{}",
        gpu_ms.histogram(16, gpu_ms.low_mean(0.001).max(0.1) * 1.5)
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The census must never report the opaque set alone — the arenas are shared
    /// pools, so dropping translucent understates both totals and utilization.
    #[test]
    fn summary_includes_translucent_geometry() {
        let mut s = GeomSummary::default();
        s.add_column(100, 150, 0, 0); // opaque-only column
        s.add_column(10, 15, 40, 60); // column with BOTH sets
        assert_eq!(s.uploaded_columns, 2);
        assert_eq!(s.opaque_verts, 110);
        assert_eq!(s.opaque_indices, 165);
        assert_eq!(s.trans_verts, 40);
        assert_eq!(s.trans_indices, 60);
        assert_eq!(
            s.total_verts(),
            150,
            "translucent verts must be in the total"
        );
        assert_eq!(
            s.total_indices(),
            225,
            "translucent indices must be in the total"
        );
        // Utilization is computed from the COMBINED totals against the shared pools.
        assert!(
            (s.vert_utilization() - 150.0 / rewo_gpu::world::MAX_VERTS as f64).abs() < 1e-12,
            "vertex utilization must use opaque+translucent"
        );
        assert!(
            (s.index_utilization() - 225.0 / rewo_gpu::world::MAX_INDICES as f64).abs() < 1e-12,
            "index utilization must use opaque+translucent"
        );
        // An opaque-only reading would land here — explicitly rejected.
        assert_ne!(s.total_verts(), s.opaque_verts);
        assert_ne!(s.total_indices(), s.opaque_indices);
    }

    /// Byte accounting uses the real `MeshVertex` size and 4-byte indices, and
    /// must agree with the GPU arena stride.
    #[test]
    fn byte_accounting_uses_real_vertex_size_and_u32_indices() {
        assert_eq!(
            VERTEX_BYTES,
            std::mem::size_of::<rewo_mesh::MeshVertex>() as u64
        );
        assert_eq!(INDEX_BYTES, 4);
        assert_eq!(
            VERTEX_BYTES,
            rewo_gpu::world::VERTEX_STRIDE,
            "size_of::<MeshVertex>() must equal the GPU upload stride"
        );

        let mut s = GeomSummary::default();
        s.add_column(2, 3, 5, 7); // 7 verts, 10 indices combined
        let expect = 7 * VERTEX_BYTES + 10 * INDEX_BYTES;
        assert_eq!(s.total_bytes(), expect);
        // The two classic mistakes: sizing indices like vertices, and dropping
        // the index term altogether.
        assert_ne!(s.total_bytes(), 17 * VERTEX_BYTES);
        assert_ne!(s.total_bytes(), 7 * VERTEX_BYTES);
    }

    /// The per-column maximum is a capacity question, so it tracks the combined
    /// footprint, not whichever set happens to be larger.
    #[test]
    fn max_column_tracks_combined_not_opaque_only() {
        let mut s = GeomSummary::default();
        s.add_column(50, 60, 0, 0); // opaque-heavy
        s.add_column(10, 12, 45, 54); // combined 55 verts — the real max
        assert_eq!(s.max_column_verts, 55, "max column must combine both sets");
        assert_eq!(s.max_column_bytes, 55 * VERTEX_BYTES + 66 * INDEX_BYTES);
    }

    /// An all-air coordinate costs mesh time but yields no `ColumnMesh`, so it
    /// is absent from `uploaded_columns`. Normalizing timing by the uploaded
    /// count would silently inflate ms/column by the empty fraction; the divisor
    /// must be the attempted count.
    #[test]
    fn empty_columns_do_not_corrupt_timing_normalization() {
        let mut s = GeomSummary::default();
        s.add_column(10, 15, 0, 0);
        s.add_column(10, 15, 0, 0);
        assert_eq!(s.uploaded_columns, 2, "only non-empty columns are counted");

        // The timed loop walked 4 coordinates; 2 meshed to nothing.
        let attempted = 4usize;
        let total_ms = 100.0f32;
        assert_eq!(ms_per_column(total_ms, attempted), 25.0);
        // Using the uploaded count would report 50.0 — a 2x overstatement.
        assert_ne!(
            ms_per_column(total_ms, attempted),
            ms_per_column(total_ms, s.uploaded_columns as usize),
            "ms/column must divide by attempted, not by non-empty"
        );
        // Column-arena utilization, by contrast, correctly uses uploaded slots:
        // an empty column consumes no metadata slot.
        assert!((s.column_utilization() - 2.0 / rewo_gpu::world::MAX_COLUMNS as f64).abs() < 1e-12);
        // Degenerate input must not divide by zero.
        assert_eq!(ms_per_column(100.0, 0), 100.0);
    }

    /// A `ColumnMesh` carrying the given geometry lengths and greedy census.
    /// Only the *lengths* reach the summary, so the vertices are zeroed — this
    /// keeps the fixture cheap even when the census numbers are huge.
    fn mesh_fixture(
        (ov, oi, tv, ti): (usize, usize, usize, usize),
        (visible, candidates, quads, fallback): (u32, u32, u32, u32),
    ) -> rewo_mesh::ColumnMesh {
        use bytemuck::Zeroable;
        rewo_mesh::ColumnMesh {
            cx: 0,
            cz: 0,
            vertices: vec![rewo_mesh::MeshVertex::zeroed(); ov],
            indices: vec![0u32; oi],
            tvertices: vec![rewo_mesh::MeshVertex::zeroed(); tv],
            tindices: vec![0u32; ti],
            y_min: 0.0,
            y_max: 0.0,
            visible_cube_faces: visible,
            greedy_candidate_faces: candidates,
            greedy_quads: quads,
            unit_fallback_faces: fallback,
            carried_fluid_cells: 0,
        }
    }

    /// Every uploaded mesh must reach the greedy census. A fold that walks the
    /// meshes but skips one reports fewer visible faces — which reads as *less*
    /// geometry, not as a bug — so the omission is pinned two ways: against the
    /// true total, and against the value a skipped-last-column bug would give.
    #[test]
    fn greedy_census_sums_every_uploaded_column() {
        let meshes = [
            mesh_fixture((10, 15, 0, 0), (100, 60, 20, 40)),
            mesh_fixture((20, 30, 5, 9), (200, 150, 30, 50)),
            mesh_fixture((1, 3, 0, 0), (7, 4, 1, 3)),
        ];
        let s = GeomSummary::from_meshes(&meshes);

        assert_eq!(s.uploaded_columns, 3);
        assert_eq!(
            s.greedy_columns, 3,
            "every uploaded column must also be folded into the face census"
        );
        assert_eq!(s.visible_cube_faces, 307);
        assert_eq!(s.greedy_candidate_faces, 214);
        assert_eq!(s.greedy_quads, 51);
        assert_eq!(s.unit_fallback_faces, 93);
        assert!(s.check_greedy_invariants().is_ok());

        // What dropping the last column would report — explicitly rejected.
        let short = GeomSummary::from_meshes(&meshes[..2]);
        assert_eq!(short.visible_cube_faces, 300);
        assert_ne!(
            s.visible_cube_faces, short.visible_cube_faces,
            "a skipped column must change the total, not pass silently"
        );
        assert_ne!(s.greedy_quads, short.greedy_quads);

        // Geometry folded but census skipped: the column-count guard catches it.
        let mut desynced = GeomSummary::default();
        for m in &meshes {
            desynced.add_column(m.vertices.len() as u64, m.indices.len() as u64, 0, 0);
        }
        desynced.add_greedy(100, 60, 20, 40);
        desynced.add_greedy(200, 150, 30, 50);
        assert!(
            desynced.check_greedy_invariants().is_err(),
            "a column present in the geometry fold but absent from the census must be rejected"
        );
    }

    /// The per-column invariants `rewo-mesh` asserts survive summation, so the
    /// aggregate must reject any census that violates them.
    #[test]
    fn greedy_invariants_reject_an_impossible_census() {
        let mut ok = GeomSummary::default();
        ok.add_column(1, 1, 0, 0);
        ok.add_greedy(100, 60, 60, 40);
        assert!(ok.check_greedy_invariants().is_ok());
        // The boundary is inclusive: every candidate may emit its own rectangle.
        assert_eq!(ok.candidate_quad_reduction_percent(), 0.0);

        // visible != candidates + fallback (one face counted in neither bucket).
        let mut split = GeomSummary::default();
        split.add_column(1, 1, 0, 0);
        split.add_greedy(100, 60, 10, 39);
        let err = split.check_greedy_invariants().unwrap_err();
        assert!(err.contains("visible cube faces"), "unexpected: {err}");

        // More rectangles than candidates — impossible, a rectangle consumes
        // at least one candidate face.
        let mut over = GeomSummary::default();
        over.add_column(1, 1, 0, 0);
        over.add_greedy(100, 60, 61, 40);
        let err = over.check_greedy_invariants().unwrap_err();
        assert!(err.contains("rectangles"), "unexpected: {err}");
    }

    /// Reduction is charged against **candidates**. Unit fallbacks never
    /// entered the merge, so measuring against visible faces (or reporting the
    /// surviving fraction instead of the reduction) misstates the pass.
    #[test]
    fn candidate_quad_reduction_uses_candidates_as_the_denominator() {
        let mut s = GeomSummary::default();
        s.add_column(1, 1, 0, 0);
        s.add_greedy(1600, 1000, 250, 600);
        assert!(s.check_greedy_invariants().is_ok());

        let pct = s.candidate_quad_reduction_percent();
        assert!(
            (pct - 75.0).abs() < 1e-9,
            "1000 → 250 is a 75% reduction, got {pct}"
        );
        // Against all visible faces (fallbacks included) this would read 84.4%.
        assert!(
            (pct - 100.0 * (1.0 - 250.0 / 1600.0)).abs() > 1.0,
            "fallbacks must not be in the denominator"
        );
        // Reporting the surviving fraction rather than the reduction gives 25%.
        assert!(
            (pct - 25.0).abs() > 1.0,
            "must report the reduction, not the remainder"
        );

        // No candidates → 0.0, never a 0/0 NaN in the report line.
        let empty = GeomSummary::default();
        let pct = empty.candidate_quad_reduction_percent();
        assert!(pct.is_finite() && pct == 0.0);
    }

    /// The per-column counters are `u32`; the totals are `u64`. A replay large
    /// enough to overflow `u32` must not wrap — a wrapped total is *smaller*,
    /// so it would read as better merging instead of as corruption.
    #[test]
    fn face_totals_widen_past_u32() {
        let meshes = [
            mesh_fixture(
                (1, 1, 0, 0),
                (u32::MAX, 4_000_000_000, 1_000_000_000, 294_967_295),
            ),
            mesh_fixture(
                (1, 1, 0, 0),
                (u32::MAX, 4_000_000_000, 1_000_000_000, 294_967_295),
            ),
        ];
        let s = GeomSummary::from_meshes(&meshes);
        assert_eq!(s.visible_cube_faces, 2 * u32::MAX as u64);
        assert!(
            s.visible_cube_faces > u32::MAX as u64,
            "totals must widen, not wrap"
        );
        assert_eq!(s.greedy_candidate_faces, 8_000_000_000);
        assert_eq!(s.greedy_quads, 2_000_000_000);
        assert!(s.check_greedy_invariants().is_ok());
        assert!((s.candidate_quad_reduction_percent() - 75.0).abs() < 1e-9);
    }

    #[test]
    fn min_med_max_handles_even_and_odd_samples() {
        assert_eq!(min_med_max(&[]), (0.0, 0.0, 0.0));
        assert_eq!(min_med_max(&[5.0]), (5.0, 5.0, 5.0));
        assert_eq!(min_med_max(&[3.0, 1.0, 2.0]), (1.0, 2.0, 3.0));
        // Even count → mean of the middle pair.
        assert_eq!(min_med_max(&[4.0, 1.0, 3.0, 2.0]), (1.0, 2.5, 4.0));
    }
}
