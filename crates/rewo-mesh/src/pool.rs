//! The mesh worker pool — REWO_PLAN §4's "meshing happens off the frame".
//!
//! `MeshPool` runs `mesh_column` on a dedicated rayon pool. Each job gets a
//! [`World::snapshot_3x3`] — 9 `Arc<Column>` clones, no block data copied —
//! so workers read a stable view while the main thread keeps applying
//! packets (copy-on-write via `Arc::make_mut` on the write side).
//!
//! Ordering safety comes from one rule: **a column already in flight at the
//! same generation is never resubmitted** (`submit` returns `false`; the
//! caller keeps it dirty and resubmits after the result lands). That gives
//! per-column ordering within a generation. Staleness converges the same way
//! it does for neighbor loads: whatever re-dirtied the column is still
//! recorded, so a fresh snapshot follows the stale result.
//!
//! Across generations the rule is different, and that is the point of the
//! `generation` field: in-flight identity is `(generation, cx, cz)`, so a
//! dimension change may resubmit the very same coordinate while the old
//! world's job is still running. Both jobs complete and both are reported;
//! each `MeshOutput` carries the generation it was meshed at so the caller
//! can drop results from a world that no longer exists — the pool never
//! silently drops or reorders them.
//!
//! `mesh_all` is the one-shot companion for snapshot renders (`view`,
//! `live --out`): parallel over the caller's `&World` directly (no
//! snapshots needed — nothing mutates during a one-shot), order-preserving.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rewo_data::assets::{CarriedFluid, CullInfo, Quad, RenderKind};
use rewo_world::World;

use crate::{mesh_column_with, ColumnMesh, MeshInputs};

/// The baked per-state render tables the mesher reads — owned once by the
/// pool, shared read-only with every worker.
pub struct MeshTables {
    pub render: Vec<RenderKind>,
    pub models: Vec<Vec<Quad>>,
    /// Per-state carried water (M164) — `BakedAssets::fluid`. An empty vec is
    /// legal and means "no block carries a fluid", which is what every
    /// pre-M164 fixture asserts; it is NOT how a production caller opts out,
    /// because the way this feature disappears is precisely a caller that
    /// forgot it.
    pub fluid: Vec<Option<CarriedFluid>>,
    /// Per-state face culling (`BakedAssets::cull`). Empty = the legacy rule
    /// (a `RenderKind::Cube` occludes, nothing is translucent).
    pub cull: Vec<CullInfo>,
    /// Per-state light emission / dampening (`BakedAssets::emission`,
    /// `::dampening`) — what smooth lighting reads. Empty = the legacy rule.
    pub emission: Vec<u8>,
    pub dampening: Vec<u8>,
    /// Vanilla's "Smooth Lighting" option.
    pub smooth_lighting: bool,
}

impl MeshTables {
    /// Every table the mesher reads, cloned from a bake, smooth lighting on.
    /// Production callers use this so a table added later cannot be left out.
    pub fn from_baked(b: &rewo_data::assets::BakedAssets) -> Self {
        let inputs = MeshInputs::from_baked(b);
        Self {
            render: b.render.clone(),
            models: b.models.clone(),
            fluid: b.fluid.clone(),
            cull: b.cull.clone(),
            emission: b.emission.clone(),
            dampening: b.dampening.clone(),
            smooth_lighting: inputs.smooth_lighting,
        }
    }

    fn inputs(&self) -> MeshInputs<'_> {
        MeshInputs {
            render: &self.render,
            models: &self.models,
            fluid: &self.fluid,
            cull: &self.cull,
            emission: &self.emission,
            dampening: &self.dampening,
            smooth_lighting: self.smooth_lighting,
        }
    }
}

/// One finished mesh job. `mesh: None` means the column baked to nothing
/// (all air / invisible / column gone) — the caller should drop any GPU
/// buffers it holds for that column.
pub struct MeshOutput {
    /// The world generation this mesh was baked from — exactly the value the
    /// matching `submit` was given. The caller compares it against the live
    /// generation and discards mismatches (post-dimension-change staleness).
    pub generation: u64,
    pub cx: i32,
    pub cz: i32,
    pub mesh: Option<ColumnMesh>,
}

pub struct MeshPool {
    pool: rayon::ThreadPool,
    tables: Arc<MeshTables>,
    /// Vanilla's "Smooth Lighting" option (`Options.ambientOcclusion`), seeded
    /// from [`MeshTables::smooth_lighting`] and moved per job at submit time —
    /// see [`Self::set_smooth_lighting`].
    smooth_lighting: bool,
    tx: Sender<MeshOutput>,
    rx: Receiver<MeshOutput>,
    /// Identity is `(generation, cx, cz)` — a new generation may re-enter a
    /// coordinate whose old-world job has not landed yet.
    in_flight: HashSet<(u64, i32, i32)>,
}

impl MeshPool {
    /// Worker count: leave headroom for the render loop + the socket reader
    /// thread, aiming at the plan's "~physical cores" on an SMT machine.
    fn default_threads() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8)
            .saturating_sub(2)
            .clamp(1, 8)
    }

    pub fn new(tables: MeshTables) -> Result<Self, String> {
        let threads = Self::default_threads();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("rewo-mesh-{i}"))
            .build()
            .map_err(|e| format!("mesh pool: {e}"))?;
        log::info!("mesh pool: {threads} workers");
        let (tx, rx) = mpsc::channel();
        let smooth_lighting = tables.smooth_lighting;
        Ok(Self {
            pool,
            tables: Arc::new(tables),
            smooth_lighting,
            tx,
            rx,
            in_flight: HashSet::new(),
        })
    }

    /// Vanilla's "Smooth Lighting" option (`Options.ambientOcclusion`), on by
    /// default.
    ///
    /// Takes effect at the next [`Self::submit`]: the value is read there and
    /// moved into the job, so a job already in flight keeps the setting it was
    /// submitted with (nothing can change under it) and every job submitted
    /// from here on meshes the new way. A caller that flips this is expected
    /// to re-mesh every loaded column — vanilla's `allChanged` on the option's
    /// `onValueUpdate` — and those re-submits are what carry the new value.
    pub fn set_smooth_lighting(&mut self, on: bool) {
        self.smooth_lighting = on;
    }

    /// Queue a mesh job for (cx, cz) at `generation`. Returns `false` —
    /// without submitting — when a job for that *same* generation and column
    /// is already in flight; the caller should keep the column dirty and
    /// retry after draining the result. A different generation for the same
    /// column is always accepted: it describes a different world.
    pub fn submit(&mut self, generation: u64, world: &World, cx: i32, cz: i32) -> bool {
        if !self.in_flight.insert((generation, cx, cz)) {
            return false;
        }
        let snapshot = world.snapshot_3x3(cx, cz);
        let tables = Arc::clone(&self.tables);
        let smooth_lighting = self.smooth_lighting;
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            let mesh = mesh_column_with(
                &snapshot,
                MeshInputs { smooth_lighting, ..tables.inputs() },
                cx,
                cz,
            );
            // Receiver gone = app is shutting down; the result is moot.
            let _ = tx.send(MeshOutput {
                generation,
                cx,
                cz,
                mesh,
            });
        });
        true
    }

    /// Take one finished job, if any (never blocks).
    pub fn try_recv(&mut self) -> Option<MeshOutput> {
        let out = self.rx.try_recv().ok()?;
        self.in_flight.remove(&(out.generation, out.cx, out.cz));
        Some(out)
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }
}

/// Mesh every coordinate in parallel over the caller's world, preserving
/// `coords` order in the output (rayon's ordered collect). One-shot use
/// only — the world must not be mutated while this runs (the `&World`
/// borrow enforces exactly that).
///
/// `generation` is stamped onto every output purely so one-shot results are
/// indistinguishable from pool results downstream; a one-shot cannot go
/// stale (nothing can change the world mid-call), so callers with no
/// generation of their own may pass 0.
pub fn mesh_all(
    generation: u64,
    world: &World,
    render: &[RenderKind],
    models: &[Vec<Quad>],
    fluid: &[Option<CarriedFluid>],
    coords: &[(i32, i32)],
) -> Vec<MeshOutput> {
    let inputs = MeshInputs::geometry(render, models, fluid);
    mesh_all_with(generation, world, inputs, coords)
}

/// [`mesh_all`] over full [`MeshInputs`].
pub fn mesh_all_with(
    generation: u64,
    world: &World,
    inputs: MeshInputs<'_>,
    coords: &[(i32, i32)],
) -> Vec<MeshOutput> {
    coords
        .par_iter()
        .map(|&(cx, cz)| MeshOutput {
            generation,
            cx,
            cz,
            mesh: mesh_column_with(world, inputs, cx, cz),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_world::dimension::DimensionShape;

    /// Tiny synthetic table: state 0 = air, state 1 = a full cube.
    fn tables() -> MeshTables {
        MeshTables {
            render: vec![
                RenderKind::Invisible,
                RenderKind::Cube {
                    faces: [0; 6],
                    raw_faces: [0; 6],
                    tint: [rewo_data::assets::TintSource::None; 6],
                },
            ],
            models: Vec::new(),
            fluid: Vec::new(),
            cull: Vec::new(),
            emission: Vec::new(),
            dampening: Vec::new(),
            smooth_lighting: true,
        }
    }

    fn one_block_world() -> World {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 10, 4, 1);
        w
    }

    fn recv_blocking(pool: &mut MeshPool) -> MeshOutput {
        for _ in 0..2000 {
            if let Some(out) = pool.try_recv() {
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("mesh pool never returned a result");
    }

    #[test]
    fn pool_meshes_a_column_off_thread() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let world = one_block_world();
        assert!(pool.submit(7, &world, 0, 0));
        assert_eq!(pool.in_flight(), 1);
        let out = recv_blocking(&mut pool);
        assert_eq!((out.generation, out.cx, out.cz), (7, 0, 0));
        // One lone cube, no neighbors: all 6 faces × 4 verts.
        let mesh = out.mesh.expect("one block must mesh");
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(pool.in_flight(), 0);
    }

    #[test]
    fn duplicate_submit_is_refused_until_result_drained() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let world = one_block_world();
        assert!(pool.submit(3, &world, 0, 0));
        assert!(
            !pool.submit(3, &world, 0, 0),
            "in-flight column must be refused at the same generation"
        );
        let _ = recv_blocking(&mut pool);
        assert!(pool.submit(3, &world, 0, 0), "drained column must resubmit");
        let _ = recv_blocking(&mut pool);
    }

    /// The M16 rule: a dimension change may re-enter a coordinate whose old
    /// job is still in flight. Both are accepted, both land, and each output
    /// names the generation it was meshed at.
    #[test]
    fn new_generation_reenters_a_column_still_in_flight() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let world = one_block_world();
        assert!(pool.submit(1, &world, 0, 0));
        assert!(
            !pool.submit(1, &world, 0, 0),
            "same generation + coord must still be refused"
        );
        // Not drained yet — a *different* generation is nonetheless accepted.
        assert!(
            pool.submit(2, &world, 0, 0),
            "new generation must be accepted for an in-flight coord"
        );
        assert_eq!(pool.in_flight(), 2);

        let a = recv_blocking(&mut pool);
        let b = recv_blocking(&mut pool);
        assert_eq!((a.cx, a.cz), (0, 0));
        assert_eq!((b.cx, b.cz), (0, 0));
        let mut gens = [a.generation, b.generation];
        gens.sort_unstable();
        assert_eq!(gens, [1, 2], "both generations must be reported");
        assert_eq!(pool.in_flight(), 0, "exact identities must be removed");
    }

    #[test]
    fn snapshot_isolates_workers_from_later_edits() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let mut world = one_block_world();
        assert!(pool.submit(0, &world, 0, 0));
        // Mutate AFTER the snapshot was taken — the in-flight job must not
        // see it (copy-on-write), and the world must see it immediately.
        world.set_block(4, 11, 4, 1);
        let out = recv_blocking(&mut pool);
        assert_eq!(out.mesh.expect("meshed").vertices.len(), 24);
        assert_eq!(world.block_state_at(4, 11, 4), 1);
    }

    /// The four `(pos, color)` of the base block's up face (y = 11, x/z in
    /// 4..=5), sorted by position. Exactly one quad qualifies — the raised
    /// block's own faces reach outside that box.
    fn up_face(mesh: &ColumnMesh) -> Vec<([f32; 3], [u8; 3])> {
        let mut face: Vec<([f32; 3], [u8; 3])> = mesh
            .vertices
            .chunks_exact(4)
            .filter(|q| {
                q.iter().all(|v| {
                    v.pos[1] == 11.0
                        && (4.0..=5.0).contains(&v.pos[0])
                        && (4.0..=5.0).contains(&v.pos[2])
                })
            })
            .flatten()
            .map(|v| (v.pos, v.color_rgb()))
            .collect();
        assert_eq!(face.len(), 4, "exactly one up face in the box: {face:?}");
        face.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        face
    }

    /// **`set_smooth_lighting` is read at submit time and reaches the mesher.**
    ///
    /// A block with a raised diagonal neighbour shades the up face's nearest
    /// corner (vanilla's four-sample AO: that corner's sample is the raised
    /// block, `shade_brightness` 0.2, so `(1 + 1 + 0.2 + 1) / 4 = 0.8`);
    /// with the option off the same face is `tesselateFlat`'s `gray(shade)` —
    /// all four vertex colors equal. The pool starts at the TABLES' value, and
    /// the setter flips both ways for every job submitted after it.
    #[test]
    fn set_smooth_lighting_is_read_at_submit_time() {
        let mut world = one_block_world();
        // One up and one diagonal over the base block: the up face's
        // (max_x, max_z) corner samples exactly this cell and no other corner
        // does.
        world.set_block(5, 11, 5, 1);
        let mut pool = MeshPool::new(tables()).unwrap(); // smooth lighting ON

        assert!(pool.submit(0, &world, 0, 0));
        let smooth = up_face(&recv_blocking(&mut pool).mesh.expect("meshed"));
        let near = smooth.iter().find(|(p, _)| *p == [5.0, 11.0, 5.0]).unwrap().1;
        let far = smooth.iter().find(|(p, _)| *p == [4.0, 11.0, 4.0]).unwrap().1;
        assert_eq!(near, [204; 3], "the corner nearest the raised block: {smooth:?}");
        assert_eq!(far, [255; 3], "the far corner is unoccluded: {smooth:?}");

        pool.set_smooth_lighting(false);
        assert!(pool.submit(0, &world, 0, 0));
        let flat = up_face(&recv_blocking(&mut pool).mesh.expect("meshed"));
        assert!(
            flat.iter().all(|(_, c)| *c == flat[0].1),
            "flat lighting: every vertex color equal — {flat:?}"
        );
        assert!(
            flat.iter().all(|(_, c)| *c == [255; 3]),
            "and `gray(up shade)` — {flat:?}"
        );

        // And back: the setter is live in both directions, per submit.
        pool.set_smooth_lighting(true);
        assert!(pool.submit(0, &world, 0, 0));
        assert_eq!(
            up_face(&recv_blocking(&mut pool).mesh.expect("meshed")),
            smooth,
            "the same job shape comes back"
        );
    }

    #[test]
    fn mesh_all_preserves_coord_order() {
        let mut world = World::new(DimensionShape::OVERWORLD);
        for cx in 0..4 {
            world.ensure_column(cx, 0);
            world.set_block(cx * 16 + 1, 5, 1, 1);
        }
        let t = tables();
        let coords = vec![(3, 0), (1, 0), (2, 0), (0, 0)];
        let outs = mesh_all(5, &world, &t.render, &t.models, &t.fluid, &coords);
        let got: Vec<(i32, i32)> = outs.iter().map(|o| (o.cx, o.cz)).collect();
        assert_eq!(got, coords);
        assert!(outs.iter().all(|o| o.mesh.is_some()));
        assert!(outs.iter().all(|o| o.generation == 5));
    }
}
