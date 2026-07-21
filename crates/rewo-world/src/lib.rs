//! rewo-world — authoritative world state decoded from the wire.
//!
//! M1 scope: paletted chunk sections, columns keyed by dimension height,
//! block-light/sky-light arrays, an entity table, a `block_state_at` query,
//! and a deterministic world digest (for replay-equivalence in the DoD).
//! Prediction/physics land in M3; this is the read model.

pub mod chunk;
pub mod dimension;
pub mod entities;
pub mod palette;
pub mod physics;

use std::collections::HashMap;

use dimension::DimensionShape;

/// The whole client-visible world for one dimension.
pub struct World {
    pub shape: DimensionShape,
    columns: HashMap<(i32, i32), chunk::Column>,
    pub entities: entities::EntityTable,
}

impl World {
    pub fn new(shape: DimensionShape) -> Self {
        Self {
            shape,
            columns: HashMap::new(),
            entities: entities::EntityTable::default(),
        }
    }

    pub fn insert_column(&mut self, cx: i32, cz: i32, column: chunk::Column) {
        self.columns.insert((cx, cz), column);
    }

    /// Ensure an all-air, fully-lit column exists (synthetic scenes).
    pub fn ensure_column(&mut self, cx: i32, cz: i32) {
        self.columns
            .entry((cx, cz))
            .or_insert_with(|| chunk::Column::empty_lit(&self.shape, cx, cz));
    }

    pub fn forget_column(&mut self, cx: i32, cz: i32) {
        self.columns.remove(&(cx, cz));
    }

    pub fn loaded_columns(&self) -> usize {
        self.columns.len()
    }

    /// Global block state id at world coords, or 0 (air) if unloaded /
    /// out of vertical range.
    pub fn block_state_at(&self, x: i32, y: i32, z: i32) -> u32 {
        let cx = x >> 4;
        let cz = z >> 4;
        let Some(col) = self.columns.get(&(cx, cz)) else {
            return 0;
        };
        col.block_state_at(&self.shape, x & 15, y, z & 15)
    }

    /// True when the column holding (x,z) is loaded.
    pub fn is_loaded(&self, x: i32, z: i32) -> bool {
        self.columns.contains_key(&(x >> 4, z >> 4))
    }

    /// Combined light level 0..15 at world coords (max of block + sky).
    /// Unloaded or above-world positions read as full-bright.
    pub fn brightness_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let Some(col) = self.columns.get(&(x >> 4, z >> 4)) else {
            return 15;
        };
        col.brightness_at(&self.shape, x & 15, y, z & 15)
    }

    pub fn column(&self, cx: i32, cz: i32) -> Option<&chunk::Column> {
        self.columns.get(&(cx, cz))
    }

    pub fn column_coords(&self) -> Vec<(i32, i32)> {
        self.columns.keys().copied().collect()
    }

    /// Apply a single block change (Block Update packet).
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let cx = x >> 4;
        let cz = z >> 4;
        if let Some(col) = self.columns.get_mut(&(cx, cz)) {
            col.set_block(&self.shape, x & 15, y, z & 15, state);
        }
    }

    /// Order-independent digest of all loaded block states — two clients
    /// (live vs replay) that saw the same packets must agree. Uses a
    /// commutative fold so column insertion order can't change the result.
    pub fn digest(&self) -> u64 {
        let mut acc: u64 = 0;
        for ((cx, cz), col) in &self.columns {
            let mut h = 1469598103934665603u64; // FNV offset
            fnv(&mut h, *cx as u64);
            fnv(&mut h, *cz as u64);
            h = col.digest(&self.shape, h);
            acc = acc.wrapping_add(h); // commutative across columns
        }
        acc
    }
}

pub(crate) fn fnv(h: &mut u64, v: u64) {
    for i in 0..8 {
        *h ^= (v >> (i * 8)) & 0xff;
        *h = h.wrapping_mul(1099511628211);
    }
}
