//! The client-side light engine.
//!
//! A vanilla client does **not** get light from the server for its own edits:
//! `chunk_data` carries authoritative light at load time, but placing a torch
//! or breaking a roof produces no light packet. The client recomputes locally,
//! and so must Rewo — without this, a torch placed at night lights nothing and
//! a dug tunnel stays at whatever the server's load-time snapshot said.
//!
//! # The vanilla rule (transcribed, not guessed)
//!
//! From the decompiled `BlockBehaviour.getLightDampening`:
//!
//! ```text
//! dampening = isSolidRender ? 15 : (propagatesSkylightDown ? 0 : 1)
//! propagatesSkylightDown = !fullCubeShape && fluidState.isEmpty()
//! ```
//!
//! and from `LightEngine`, the cost of stepping *into* a cell is
//! `max(1, dampening)`. Both facts are baked per state into
//! [`LightTables`] by `rewo-data` — see `assets::bake`.
//!
//! Sky sources come from `ChunkSkyLightSources.isEdgeOccluded`: the open
//! column descends while `dampening == 0`, and every cell in it is a level-15
//! source. Seeding the whole column (rather than special-casing "downward
//! propagation does not attenuate") makes the two channels share one BFS.
//!
//! # Algorithm
//!
//! The classic two-phase flood fill, one pair of queues per channel:
//!
//! * **decrease** — pop `(pos, old)`. A neighbour dimmer than `old` was lit by
//!   us: zero it and cascade. A neighbour at least as bright has another
//!   source: re-seed it into the increase queue so it fills the hole back in.
//! * **increase** — pop `pos`. Push `light(pos) - max(1, dampening(n))` into
//!   each neighbour that is currently darker.
//!
//! Decrease runs to completion first, then increase; that ordering is what
//! makes removal exact rather than leaving stale halos.
//!
//! # Bounds
//!
//! Propagation stops at unloaded columns — an edit near the render-distance
//! edge simply does not light cells the client cannot see. This is both the
//! correct client behaviour and the runaway-cost guard.

use std::collections::{HashSet, VecDeque};

use crate::World;

/// Per-block-state light properties, indexed by state id.
///
/// Passed in rather than depended on so `rewo-world` stays free of
/// `rewo-data` — the same seam `physics::tick` uses for collision shapes.
#[derive(Clone, Copy)]
pub struct LightTables<'a> {
    /// Light the state emits, 0..15.
    pub emission: &'a [u8],
    /// Light the state absorbs, 0..15. Step cost is `max(1, dampening)`.
    pub dampening: &'a [u8],
    /// Per-state bitmask of fully-covered faces, in [`NEIGHBOURS`] order.
    /// A face that either side fully covers passes no light at all — vanilla's
    /// `getLightDampeningInto` returns 16 for it. This is what makes a stair
    /// or a slab shadow correctly despite having dampening 0.
    pub face_occludes: &'a [u8],
}

impl LightTables<'_> {
    fn emission(&self, state: u32) -> u8 {
        self.emission.get(state as usize).copied().unwrap_or(0)
    }

    fn dampening(&self, state: u32) -> u8 {
        self.dampening.get(state as usize).copied().unwrap_or(0)
    }

    /// Whether light may cross from `from` into `to` through face `dir`
    /// (an index into [`NEIGHBOURS`]).
    ///
    /// Vanilla merges the two occlusion shapes and blocks the step when they
    /// together cover the face. Testing each side independently covers every
    /// shape in the game whose face is covered at all — the merge only differs
    /// for two *complementary partial* faces meeting (a bottom slab under a top
    /// slab), which no vanilla pair produces.
    fn blocked(&self, from: u32, to: u32, dir: usize) -> bool {
        let f = self.face_occludes.get(from as usize).copied().unwrap_or(0);
        let t = self.face_occludes.get(to as usize).copied().unwrap_or(0);
        f & (1 << dir) != 0 || t & (1 << (dir ^ 1)) != 0
    }
}

/// Which of the two light channels a pass operates on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Block,
    Sky,
}

/// The six neighbour offsets. Order matters twice: it matches
/// `rewo_data::assets::FACE_DIRS` so a face bitmask indexes straight in, and
/// opposite faces are paired as `i ^ 1` so `blocked` can flip a direction.
const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Reusable scratch for light updates. Holding the queues across calls keeps
/// a burst of edits (a dug tunnel) from reallocating per block.
#[derive(Default)]
pub struct LightEngine {
    increase: VecDeque<(i32, i32, i32)>,
    decrease: VecDeque<(i32, i32, i32, u8)>,
    /// Columns whose light changed during the current call — the caller
    /// remeshes exactly these.
    touched: HashSet<(i32, i32)>,
}

impl LightEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Relight around a block that just changed from `old` to `new`.
    ///
    /// Returns the columns whose light changed, which the caller must remesh
    /// — a light change is invisible until the mesh's baked light is rebuilt.
    /// The set always includes the edited column.
    pub fn on_block_change(
        &mut self,
        world: &mut World,
        t: LightTables,
        x: i32,
        y: i32,
        z: i32,
        old: u32,
        new: u32,
    ) -> Vec<(i32, i32)> {
        self.touched.clear();

        // -- block light ----------------------------------------------------
        // Remove whatever the old state contributed, then seed the new one.
        // The removal pass also re-seeds any neighbouring source it uncovers,
        // so "break the wall between a torch and a room" fills correctly.
        let old_level = self.get(world, Channel::Block, x, y, z);
        if old_level > 0 {
            self.set(world, Channel::Block, x, y, z, 0);
            self.decrease.push_back((x, y, z, old_level));
        }
        self.run_decrease(world, t, Channel::Block);

        let emit = t.emission(new);
        if emit > 0 {
            self.set(world, Channel::Block, x, y, z, emit);
            self.increase.push_back((x, y, z));
        }
        // A cell that became transparent must also be re-fed by its
        // neighbours; seeding them is cheaper than a separate rule.
        for (dx, dy, dz) in NEIGHBOURS {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if world.is_loaded(nx, nz) && self.get(world, Channel::Block, nx, ny, nz) > 0 {
                self.increase.push_back((nx, ny, nz));
            }
        }
        self.run_increase(world, t, Channel::Block);

        // -- sky light ------------------------------------------------------
        // Only the (x,z) column's open-sky extent can change, so recompute
        // that one column's sources and diff against what is stored.
        //
        // A dimension with `hasSkyLight == false` has no sky light engine in
        // vanilla at all: the server never sends sky data and the client never
        // computes any. Running the flood there would invent sky light out of
        // the world's zero-initialised arrays, so the whole channel is skipped.
        if world.has_sky_light() {
            self.update_sky_column(world, t, x, z, old, new, y);
            self.run_decrease(world, t, Channel::Sky);
            self.run_increase(world, t, Channel::Sky);
        }

        self.touched.iter().copied().collect()
    }

    /// Recompute a whole column's light from scratch, ignoring whatever the
    /// server sent. This is the verification path — `rewo play --relight`
    /// diffs the result against the server's authoritative values, the same
    /// way `CORRECTIONS` validates physics.
    ///
    /// Neighbour columns are read but not rewritten, so light entering from
    /// outside is respected; run it over a 3×3 to converge fully.
    pub fn relight_column(&mut self, world: &mut World, t: LightTables, cx: i32, cz: i32) {
        self.touched.clear();
        let shape = world.shape;
        let (y0, y1) = (shape.min_y, shape.min_y + shape.height);

        for lx in 0..16 {
            for lz in 0..16 {
                let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                for y in y0..y1 {
                    self.set(world, Channel::Block, x, y, z, 0);
                    self.set(world, Channel::Sky, x, y, z, 0);
                }
                // Sky sources: the open column, per vanilla's edge rule.
                let bottom = self.sky_bottom(world, t, x, z);
                for y in bottom..y1 {
                    self.set(world, Channel::Sky, x, y, z, 15);
                    self.increase.push_back((x, y, z));
                }
                // Block sources.
                for y in y0..y1 {
                    let emit = t.emission(world.block_state_at(x, y, z));
                    if emit > 0 {
                        self.set(world, Channel::Block, x, y, z, emit);
                        self.increase.push_back((x, y, z));
                    }
                }
            }
        }
        // Light entering from the neighbours: seed the four faces.
        for i in 0..16 {
            for (x, z) in [
                (cx * 16 - 1, cz * 16 + i),
                (cx * 16 + 16, cz * 16 + i),
                (cx * 16 + i, cz * 16 - 1),
                (cx * 16 + i, cz * 16 + 16),
            ] {
                if !world.is_loaded(x, z) {
                    continue;
                }
                for y in y0..y1 {
                    if self.get(world, Channel::Block, x, y, z) > 0
                        || self.get(world, Channel::Sky, x, y, z) > 0
                    {
                        self.increase.push_back((x, y, z));
                    }
                }
            }
        }
        // One increase pass per channel; the queue carries both, so run each
        // channel's flood separately over the same seeds.
        let seeds: Vec<_> = self.increase.iter().copied().collect();
        self.increase.clear();
        for ch in [Channel::Block, Channel::Sky] {
            self.increase.extend(seeds.iter().copied());
            self.run_increase(world, t, ch);
        }
    }

    /// The lowest y still reached directly by the sky at `(x, z)`.
    ///
    /// Vanilla's `ChunkSkyLightSources.isEdgeOccluded` stops the column when
    /// the cell dampens light **or** when the two cells' occlusion shapes cover
    /// the horizontal face between them — so a slab or stair ends the column
    /// even though its dampening is 0. Both callers must agree on this or the
    /// incremental path drifts from the full recompute.
    fn sky_bottom(&self, world: &World, t: LightTables, x: i32, z: i32) -> i32 {
        let shape = world.shape;
        let (y0, top) = (shape.min_y, shape.min_y + shape.height);
        let mut above = world.block_state_at(x, top - 1, z);
        for y in (y0..top).rev() {
            let here = world.block_state_at(x, y, z);
            // NEIGHBOURS index 2 is −Y: the step downward out of `above`.
            if t.dampening(here) != 0 || (y < top - 1 && t.blocked(above, here, 2)) {
                return y + 1;
            }
            above = here;
        }
        y0
    }

    /// The (x,z) column's open-sky extent may have changed. Diff the new
    /// source set against the stored light and queue the difference.
    fn update_sky_column(
        &mut self,
        world: &mut World,
        t: LightTables,
        x: i32,
        z: i32,
        old: u32,
        new: u32,
        y: i32,
    ) {
        let (old_d, new_d) = (t.dampening(old), t.dampening(new));
        if old_d == new_d {
            // The column's shape is unchanged; only the edited cell itself
            // needs re-seeding from its neighbours.
            for (dx, dy, dz) in NEIGHBOURS {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if world.is_loaded(nx, nz) && self.get(world, Channel::Sky, nx, ny, nz) > 0 {
                    self.increase.push_back((nx, ny, nz));
                }
            }
            return;
        }

        let shape = world.shape;
        let y0 = shape.min_y;
        let top = shape.min_y + shape.height;
        let bottom = self.sky_bottom(world, t, x, z);

        if new_d != 0 {
            // Sky just got blocked at `y`: everything below loses its source.
            for yy in y0..bottom.min(y + 1) {
                let cur = self.get(world, Channel::Sky, x, yy, z);
                if cur > 0 {
                    self.set(world, Channel::Sky, x, yy, z, 0);
                    self.decrease.push_back((x, yy, z, cur));
                }
            }
        } else {
            // Sky just opened up: seed the newly-open cells at full strength.
            for yy in bottom..top {
                if self.get(world, Channel::Sky, x, yy, z) < 15 {
                    self.set(world, Channel::Sky, x, yy, z, 15);
                    self.increase.push_back((x, yy, z));
                }
            }
        }
    }

    fn run_decrease(&mut self, world: &mut World, t: LightTables, ch: Channel) {
        while let Some((x, y, z, old)) = self.decrease.pop_front() {
            let here = world.block_state_at(x, y, z);
            for (dir, (dx, dy, dz)) in NEIGHBOURS.into_iter().enumerate() {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if !world.is_loaded(nx, nz) || !self.in_world(world, ny) {
                    continue;
                }
                if t.blocked(here, world.block_state_at(nx, ny, nz), dir) {
                    continue;
                }
                let cur = self.get(world, ch, nx, ny, nz);
                if cur == 0 {
                    continue;
                }
                if cur < old {
                    // We lit this cell — clear it and cascade.
                    self.set(world, ch, nx, ny, nz, 0);
                    self.decrease.push_back((nx, ny, nz, cur));
                } else {
                    // Fed by something else: it refills the hole.
                    self.increase.push_back((nx, ny, nz));
                }
            }
        }
    }

    fn run_increase(&mut self, world: &mut World, t: LightTables, ch: Channel) {
        while let Some((x, y, z)) = self.increase.pop_front() {
            let level = self.get(world, ch, x, y, z);
            if level <= 1 {
                continue;
            }
            let here = world.block_state_at(x, y, z);
            for (dir, (dx, dy, dz)) in NEIGHBOURS.into_iter().enumerate() {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if !world.is_loaded(nx, nz) || !self.in_world(world, ny) {
                    continue;
                }
                let there = world.block_state_at(nx, ny, nz);
                if t.blocked(here, there, dir) {
                    continue;
                }
                let cost = t.dampening(there).max(1);
                let Some(target) = level.checked_sub(cost) else {
                    continue;
                };
                if target > self.get(world, ch, nx, ny, nz) {
                    self.set(world, ch, nx, ny, nz, target);
                    self.increase.push_back((nx, ny, nz));
                }
            }
        }
    }

    fn in_world(&self, world: &World, y: i32) -> bool {
        y >= world.shape.min_y && y < world.shape.min_y + world.shape.height
    }

    fn get(&self, world: &World, ch: Channel, x: i32, y: i32, z: i32) -> u8 {
        let (b, s) = world.light_at(x, y, z);
        match ch {
            Channel::Block => b,
            Channel::Sky => s,
        }
    }

    fn set(&mut self, world: &mut World, ch: Channel, x: i32, y: i32, z: i32, level: u8) {
        let (cx, cz) = (x >> 4, z >> 4);
        let shape = world.shape;
        let Some(col) = world.column_mut(cx, cz) else {
            return;
        };
        if col.set_light(&shape, ch, x.rem_euclid(16), y, z.rem_euclid(16), level) {
            self.touched.insert((cx, cz));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::{DimensionShape, DimensionTypeDef};

    /// State 0 = air (dampening 0), 1 = stone (15), 2 = torch (emits 14).
    const EMISSION: &[u8] = &[0, 0, 14];
    const DAMPENING: &[u8] = &[0, 15, 0];
    const FACES: &[u8] = &[0, 0, 0];

    fn tables() -> LightTables<'static> {
        LightTables {
            emission: EMISSION,
            dampening: DAMPENING,
            face_occludes: FACES,
        }
    }

    /// A world of air with a stone lid, so sky light is not in the way.
    fn roofed_world() -> World {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                w.set_block(x, 10, z, 1);
            }
        }
        w
    }

    #[test]
    fn torch_lights_its_neighbourhood() {
        let mut w = roofed_world();
        let mut e = LightEngine::new();
        e.on_block_change(&mut w, tables(), 8, 5, 8, 0, 2);

        assert_eq!(w.light_at(8, 5, 8).0, 14, "the torch cell itself");
        assert_eq!(w.light_at(9, 5, 8).0, 13, "one step costs 1");
        assert_eq!(w.light_at(11, 5, 8).0, 11, "three steps");
        assert_eq!(w.light_at(8, 5, 8 - 14).0, 0, "falls off to nothing at 14");
    }

    #[test]
    fn removing_a_torch_removes_its_light() {
        let mut w = roofed_world();
        let mut e = LightEngine::new();
        e.on_block_change(&mut w, tables(), 8, 5, 8, 0, 2);
        w.set_block(8, 5, 8, 0);
        e.on_block_change(&mut w, tables(), 8, 5, 8, 2, 0);

        for d in 0..6 {
            assert_eq!(
                w.light_at(8 + d, 5, 8).0,
                0,
                "no stale halo {d} blocks from the removed torch"
            );
        }
    }

    #[test]
    fn two_torches_survive_one_removal() {
        // The decrease pass must re-seed the surviving source rather than
        // clearing the overlap between them.
        let mut w = roofed_world();
        let mut e = LightEngine::new();
        e.on_block_change(&mut w, tables(), 4, 5, 8, 0, 2);
        e.on_block_change(&mut w, tables(), 10, 5, 8, 0, 2);
        w.set_block(4, 5, 8, 0);
        e.on_block_change(&mut w, tables(), 4, 5, 8, 2, 0);

        assert_eq!(w.light_at(10, 5, 8).0, 14, "the surviving torch");
        assert_eq!(w.light_at(9, 5, 8).0, 13, "and its falloff");
        assert_eq!(w.light_at(4, 5, 8).0, 8, "lit only by the survivor now");
    }

    #[test]
    fn stone_blocks_light() {
        let mut w = roofed_world();
        let mut e = LightEngine::new();
        // A wall at x=9, torch at x=8: the far side is shadowed, but light
        // still wraps around through the open air above and below.
        for y in 0..10 {
            for z in 0..16 {
                w.set_block(9, y, z, 1);
            }
        }
        e.on_block_change(&mut w, tables(), 8, 5, 8, 0, 2);
        assert_eq!(w.light_at(9, 5, 8).0, 0, "inside the wall");
        assert_eq!(w.light_at(10, 5, 8).0, 0, "sealed behind a full wall");
    }

    #[test]
    fn sky_fills_an_open_column_and_a_lid_removes_it() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        let mut e = LightEngine::new();
        e.relight_column(&mut w, tables(), 0, 0);
        assert_eq!(w.light_at(8, 5, 8).1, 15, "open sky reaches the ground");

        w.set_block(8, 10, 8, 1);
        e.on_block_change(&mut w, tables(), 8, 10, 8, 0, 1);
        assert_eq!(w.light_at(8, 10, 8).1, 0, "the lid itself is dark");
        assert!(
            w.light_at(8, 9, 8).1 < 15,
            "the cell under a one-block lid is no longer a direct source"
        );
    }

    /// A Nether-shaped dimension type: the roofed, sky-light-less case. Built
    /// from `unresolved_holder` (whose fields are the Overworld's) with the two
    /// that matter here overridden, so the test states exactly what it depends
    /// on.
    fn nether_def() -> DimensionTypeDef {
        DimensionTypeDef {
            name: "minecraft:the_nether".into(),
            shape: DimensionShape::NETHER,
            has_sky_light: false,
            ..DimensionTypeDef::unresolved_holder(0)
        }
    }

    #[test]
    fn a_dimension_without_sky_light_never_seeds_the_sky_channel() {
        // The regression guard for `on_block_change`'s `has_sky_light` gate.
        // These columns are wide open — in the Overworld every cell would be a
        // level-15 sky source — but the Nether has no sky light engine at all,
        // so the channel must stay at the zero the arrays were born with.
        let mut w = World::for_dimension(&nether_def());
        assert!(!w.has_sky_light());
        w.ensure_column(0, 0);
        w.ensure_column(-1, 0);
        let mut e = LightEngine::new();

        // Edit 1: place a torch in open air. Block light floods; sky must not.
        w.set_block(8, 20, 8, 2);
        e.on_block_change(&mut w, tables(), 8, 20, 8, 0, 2);
        assert_eq!(w.light_at(8, 20, 8).0, 14, "block light still works");
        for y in [0, 20, 40, DimensionShape::NETHER.height - 1] {
            assert_eq!(
                w.light_at(8, y, 8).1,
                0,
                "sky stays unseeded at y={y} with has_sky_light = false"
            );
        }

        // Edit 2: a solid lid — the column-shape change that on the Overworld
        // path runs the sky decrease/increase passes. Still nothing to run.
        w.set_block(8, 30, 8, 1);
        e.on_block_change(&mut w, tables(), 8, 30, 8, 0, 1);
        for y in 0..DimensionShape::NETHER.height {
            assert_eq!(w.light_at(8, y, 8).1, 0, "sky still 0 at y={y}");
            assert_eq!(w.light_at(-1, y, 8).1, 0, "and in the neighbour column");
        }

        // Edit 3: break the lid again — the "sky just opened up" branch, which
        // is the one that would seed 15s into every cell of the column.
        w.set_block(8, 30, 8, 0);
        e.on_block_change(&mut w, tables(), 8, 30, 8, 1, 0);
        for y in 0..DimensionShape::NETHER.height {
            assert_eq!(w.light_at(8, y, 8).1, 0, "no 15s invented at y={y}");
        }
    }
}
