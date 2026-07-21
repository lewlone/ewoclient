//! The mesh worker pool — REWO_PLAN §4's "meshing happens off the frame".
//!
//! `MeshPool` runs `mesh_column` on a dedicated rayon pool. Each job gets a
//! [`World::snapshot_3x3`] — 9 `Arc<Column>` clones, no block data copied —
//! so workers read a stable view while the main thread keeps applying
//! packets (copy-on-write via `Arc::make_mut` on the write side).
//!
//! Ordering safety comes from one rule: **a column already in flight is
//! never resubmitted** (`submit` returns `false`; the caller keeps it dirty
//! and resubmits after the result lands). That gives per-column ordering
//! without generations or epochs. Staleness converges the same way it does
//! for neighbor loads: whatever re-dirtied the column is still recorded, so
//! a fresh snapshot follows the stale result.
//!
//! `mesh_all` is the one-shot companion for snapshot renders (`view`,
//! `live --out`): parallel over the caller's `&World` directly (no
//! snapshots needed — nothing mutates during a one-shot), order-preserving.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rewo_data::assets::{Quad, RenderKind};
use rewo_world::World;

use crate::{mesh_column, ColumnMesh};

/// The baked per-state render tables the mesher reads — owned once by the
/// pool, shared read-only with every worker.
pub struct MeshTables {
    pub render: Vec<RenderKind>,
    pub models: Vec<Vec<Quad>>,
}

/// One finished mesh job. `mesh: None` means the column baked to nothing
/// (all air / invisible / column gone) — the caller should drop any GPU
/// buffers it holds for that column.
pub struct MeshOutput {
    pub cx: i32,
    pub cz: i32,
    pub mesh: Option<ColumnMesh>,
}

pub struct MeshPool {
    pool: rayon::ThreadPool,
    tables: Arc<MeshTables>,
    tx: Sender<MeshOutput>,
    rx: Receiver<MeshOutput>,
    in_flight: HashSet<(i32, i32)>,
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
        Ok(Self {
            pool,
            tables: Arc::new(tables),
            tx,
            rx,
            in_flight: HashSet::new(),
        })
    }

    /// Queue a mesh job for (cx, cz). Returns `false` — without submitting —
    /// when a job for that column is already in flight; the caller should
    /// keep the column dirty and retry after draining the result.
    pub fn submit(&mut self, world: &World, cx: i32, cz: i32) -> bool {
        if !self.in_flight.insert((cx, cz)) {
            return false;
        }
        let snapshot = world.snapshot_3x3(cx, cz);
        let tables = Arc::clone(&self.tables);
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            let mesh = mesh_column(&snapshot, &tables.render, &tables.models, cx, cz);
            // Receiver gone = app is shutting down; the result is moot.
            let _ = tx.send(MeshOutput { cx, cz, mesh });
        });
        true
    }

    /// Take one finished job, if any (never blocks).
    pub fn try_recv(&mut self) -> Option<MeshOutput> {
        let out = self.rx.try_recv().ok()?;
        self.in_flight.remove(&(out.cx, out.cz));
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
pub fn mesh_all(
    world: &World,
    render: &[RenderKind],
    models: &[Vec<Quad>],
    coords: &[(i32, i32)],
) -> Vec<MeshOutput> {
    coords
        .par_iter()
        .map(|&(cx, cz)| MeshOutput {
            cx,
            cz,
            mesh: mesh_column(world, render, models, cx, cz),
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
                    tint: [false; 6],
                },
            ],
            models: Vec::new(),
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
        assert!(pool.submit(&world, 0, 0));
        assert_eq!(pool.in_flight(), 1);
        let out = recv_blocking(&mut pool);
        assert_eq!((out.cx, out.cz), (0, 0));
        // One lone cube, no neighbors: all 6 faces × 4 verts.
        let mesh = out.mesh.expect("one block must mesh");
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(pool.in_flight(), 0);
    }

    #[test]
    fn duplicate_submit_is_refused_until_result_drained() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let world = one_block_world();
        assert!(pool.submit(&world, 0, 0));
        assert!(!pool.submit(&world, 0, 0), "in-flight column must be refused");
        let _ = recv_blocking(&mut pool);
        assert!(pool.submit(&world, 0, 0), "drained column must resubmit");
        let _ = recv_blocking(&mut pool);
    }

    #[test]
    fn snapshot_isolates_workers_from_later_edits() {
        let mut pool = MeshPool::new(tables()).unwrap();
        let mut world = one_block_world();
        assert!(pool.submit(&world, 0, 0));
        // Mutate AFTER the snapshot was taken — the in-flight job must not
        // see it (copy-on-write), and the world must see it immediately.
        world.set_block(4, 11, 4, 1);
        let out = recv_blocking(&mut pool);
        assert_eq!(out.mesh.expect("meshed").vertices.len(), 24);
        assert_eq!(world.block_state_at(4, 11, 4), 1);
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
        let outs = mesh_all(&world, &t.render, &t.models, &coords);
        let got: Vec<(i32, i32)> = outs.iter().map(|o| (o.cx, o.cz)).collect();
        assert_eq!(got, coords);
        assert!(outs.iter().all(|o| o.mesh.is_some()));
    }
}
