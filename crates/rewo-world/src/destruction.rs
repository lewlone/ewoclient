//! `ClientLevel.destroyBlockProgress` — the crack overlay someone else's
//! mining paints on a block (M81).
//!
//! Two indexes over the same records, exactly as `ClientLevel` keeps them:
//!
//! * `destroyingBlocks` — `Int2ObjectMap<BlockDestructionProgress>`, keyed by
//!   the **breaker's entity id**. One breaker mines at most one block, so this
//!   is what makes moving to a new block retire the old crack.
//! * `destructionProgress` — `Long2ObjectMap<SortedSet<…>>`, keyed by the
//!   block position. Several breakers *can* share a block, and the renderer
//!   asks for `progresses.last()` — the set's ordering is what picks a winner.
//!
//! Three things here read backwards from the obvious:
//!
//! 1. **There is no `-1` on the wire.** The packet's stage is
//!    `readUnsignedByte`, so the server's `(byte) -1` arrives as **255**, and
//!    what removes the record is the range test `progress >= 0 && progress <
//!    10` failing — not a signed sentinel. A signed read would handle `-1` and
//!    then quietly *keep* a record for, say, 200.
//! 2. **`10` is out of range.** `DESTROY_STAGE_COUNT` is 10 and the stages are
//!    numbered 0..=9, so the exclusive bound is the stage count itself and a
//!    stage of exactly 10 is a *removal*. `BlockDestructionProgress.setProgress`
//!    clamps `> 10` down to 10 — transcribed below, and unreachable from this
//!    entry point, which is worth knowing rather than worth deleting.
//! 3. **The set is ordered by progress first, id second** — so the winner is
//!    the *furthest along*, and ties break toward the higher entity id. Java's
//!    `equals`/`hashCode` on these records are by id alone, which is why a
//!    `TreeSet` can hold two records at equal progress at all.
//!
//! The server never sends this to the breaker (`player.getId() != id`) and only
//! within 32 blocks (`< 1024.0` squared), so your own crack is client-predicted
//! and somebody else's arrives only when they are close.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// `ModelBakery.DESTROY_STAGE_COUNT` — `block/destroy_stage_0` … `_9`.
pub const DESTROY_STAGE_COUNT: i32 = 10;

/// `ClientLevel.removeBlockBreakingProgress`: the sweep runs on every 20th
/// game tick…
const SWEEP_PERIOD: i64 = 20;
/// …and drops a record whose last update is more than this many ticks old.
const STALE_AFTER: i64 = 400;

/// One `BlockDestructionProgress`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    pos: [i32; 3],
    progress: i32,
    updated_render_tick: i64,
}

/// `ClientLevel`'s pair of block-destruction indexes.
#[derive(Default, Debug)]
pub struct DestructionProgress {
    /// `destroyingBlocks` — breaker entity id → its one in-flight record.
    by_breaker: HashMap<i32, Record>,
    /// `destructionProgress` — position → the ordered `(progress, breaker id)`
    /// keys at it. A `BTreeSet` of the sort key reproduces Java's `TreeSet` of
    /// records ordered by `compareTo`; `BTreeMap` for the outer index so
    /// iteration order is stable and a rendered frame does not depend on hash
    /// seeding.
    by_pos: BTreeMap<[i32; 3], BTreeSet<(i32, i32)>>,
}

impl DestructionProgress {
    /// `ClientLevel.destroyBlockProgress(id, pos, progress)`.
    ///
    /// `progress` is the raw **unsigned** byte off the wire (0..=255); the
    /// range test is what distinguishes an update from a removal.
    pub fn set(&mut self, id: i32, pos: [i32; 3], progress: i32, game_time: i64) {
        if (0..DESTROY_STAGE_COUNT).contains(&progress) {
            // `if (entry != null) removeProgress(entry)` runs *before* the
            // progress is overwritten, so the set is keyed out by the value it
            // was inserted under. Doing it after would strand the old key.
            if let Some(old) = self.by_breaker.get(&id).copied() {
                self.unindex(old.pos, old.progress, id);
            }
            let entry = self.by_breaker.entry(id).or_insert(Record {
                pos,
                progress,
                updated_render_tick: game_time,
            });
            if entry.pos != pos {
                // A breaker that moved to a different block gets a fresh
                // record; the old one has already left the position index.
                *entry = Record {
                    pos,
                    progress,
                    updated_render_tick: game_time,
                };
            }
            // `setProgress` clamps above 10. Unreachable through this door —
            // the range test above already excluded it — and transcribed
            // because it is the record's own invariant, not this caller's.
            entry.progress = progress.min(DESTROY_STAGE_COUNT);
            entry.updated_render_tick = game_time;
            let (p, pos) = (entry.progress, entry.pos);
            self.by_pos.entry(pos).or_default().insert((p, id));
        } else if let Some(removed) = self.by_breaker.remove(&id) {
            self.unindex(removed.pos, removed.progress, id);
        }
    }

    /// `removeProgress` — drop one record from the position index, and the
    /// position itself once nothing is left at it.
    fn unindex(&mut self, pos: [i32; 3], progress: i32, id: i32) {
        if let Some(set) = self.by_pos.get_mut(&pos) {
            set.remove(&(progress, id));
            if set.is_empty() {
                self.by_pos.remove(&pos);
            }
        }
    }

    /// The stage the renderer draws at `pos`, or `None` for an untouched
    /// block.
    ///
    /// `progresses.last()` on a set ordered by `(progress, id)` — the
    /// **furthest-along** breaker wins, not the first to arrive and not the
    /// most recent.
    pub fn stage_at(&self, pos: [i32; 3]) -> Option<i32> {
        self.by_pos.get(&pos)?.last().map(|&(p, _)| p)
    }

    /// Every cracked block and its winning stage, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = ([i32; 3], i32)> + '_ {
        self.by_pos
            .iter()
            .filter_map(|(pos, set)| set.last().map(|&(p, _)| (*pos, p)))
    }

    /// `ClientLevel.removeBlockBreakingProgress` — the staleness sweep.
    ///
    /// Runs only on every 20th game tick, and the threshold is strictly
    /// greater than 400, so a record refreshed at t is kept through t + 400.
    /// The gate exists because a breaker who walks away sends no "stop"
    /// packet: the record is retired by silence, not by an announcement.
    pub fn tick(&mut self, game_time: i64) {
        if game_time % SWEEP_PERIOD != 0 {
            return;
        }
        let stale: Vec<(i32, Record)> = self
            .by_breaker
            .iter()
            .filter(|(_, r)| game_time - r.updated_render_tick > STALE_AFTER)
            .map(|(id, r)| (*id, *r))
            .collect();
        for (id, r) in stale {
            self.by_breaker.remove(&id);
            self.unindex(r.pos, r.progress, id);
        }
    }

    /// Number of live breaker records — the `destroyingBlocks` size.
    pub fn breaker_count(&self) -> usize {
        self.by_breaker.len()
    }

    /// Number of distinct cracked positions.
    pub fn position_count(&self) -> usize {
        self.by_pos.len()
    }

    /// Drop everything — a dimension change discards the level and its
    /// indexes with it.
    pub fn clear(&mut self) {
        self.by_breaker.clear();
        self.by_pos.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_in_range_is_recorded_and_read_back() {
        let mut d = DestructionProgress::default();
        d.set(7, [1, 2, 3], 4, 0);
        assert_eq!(d.stage_at([1, 2, 3]), Some(4));
        assert_eq!(d.breaker_count(), 1);
    }

    #[test]
    fn the_wire_never_carries_minus_one_it_carries_255() {
        // The server writes `(byte) -1`; `readUnsignedByte` makes that 255.
        // Both are out of `0..10`, so both remove — but only one of them can
        // actually arrive.
        let mut d = DestructionProgress::default();
        d.set(7, [1, 2, 3], 4, 0);
        d.set(7, [1, 2, 3], 255, 1);
        assert_eq!(d.stage_at([1, 2, 3]), None);
        assert_eq!(d.breaker_count(), 0);
    }

    #[test]
    fn ten_is_out_of_range_and_therefore_a_removal() {
        // Stages are 0..=9. A plausible `<= 10` bound would keep a record
        // whose texture does not exist.
        let mut d = DestructionProgress::default();
        d.set(7, [0, 0, 0], 9, 0);
        assert_eq!(d.stage_at([0, 0, 0]), Some(9));
        d.set(7, [0, 0, 0], 10, 1);
        assert_eq!(d.stage_at([0, 0, 0]), None);
    }

    #[test]
    fn one_breaker_cracks_one_block_at_a_time() {
        let mut d = DestructionProgress::default();
        d.set(7, [0, 0, 0], 3, 0);
        d.set(7, [5, 0, 0], 1, 1);
        assert_eq!(d.stage_at([0, 0, 0]), None, "the old block must un-crack");
        assert_eq!(d.stage_at([5, 0, 0]), Some(1));
        assert_eq!(d.breaker_count(), 1);
        assert_eq!(d.position_count(), 1);
    }

    #[test]
    fn the_furthest_along_breaker_wins_a_shared_block() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 8, 0);
        d.set(2, [0, 0, 0], 2, 0);
        // Not "the last one to arrive" and not "the lowest id".
        assert_eq!(d.stage_at([0, 0, 0]), Some(8));
        // …and when the leader retires, the other's stage is what shows.
        d.set(1, [0, 0, 0], 255, 1);
        assert_eq!(d.stage_at([0, 0, 0]), Some(2));
    }

    #[test]
    fn a_tie_breaks_toward_the_higher_id() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 5, 0);
        d.set(9, [0, 0, 0], 5, 0);
        // Both at stage 5; `compareTo` falls through to `Integer.compare(id)`,
        // and `last()` takes the greater. Invisible in the render (equal
        // stages draw the same sprite) but it is the ordering vanilla defines.
        assert_eq!(d.stage_at([0, 0, 0]), Some(5));
        assert_eq!(d.by_pos[&[0, 0, 0]].last(), Some(&(5, 9)));
    }

    #[test]
    fn re_indexing_the_same_block_does_not_leave_a_stale_key() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 2, 0);
        d.set(1, [0, 0, 0], 6, 1);
        assert_eq!(d.stage_at([0, 0, 0]), Some(6));
        // If the old `(2, 1)` key survived, this set would hold two entries
        // and retiring the breaker would leave the stale one behind.
        assert_eq!(d.by_pos[&[0, 0, 0]].len(), 1);
        d.set(1, [0, 0, 0], 255, 2);
        assert_eq!(d.position_count(), 0);
    }

    #[test]
    fn the_sweep_runs_only_on_a_multiple_of_twenty() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 3, 0);
        // Far past the threshold, but not a sweep tick.
        d.tick(401);
        assert_eq!(d.breaker_count(), 1);
        d.tick(420);
        assert_eq!(d.breaker_count(), 0);
        assert_eq!(d.position_count(), 0);
    }

    #[test]
    fn the_staleness_threshold_is_strictly_greater_than_400() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 3, 0);
        d.tick(400);
        assert_eq!(d.breaker_count(), 1, "exactly 400 old is still fresh");
        d.tick(420);
        assert_eq!(d.breaker_count(), 0);
    }

    #[test]
    fn a_refresh_resets_the_staleness_clock() {
        let mut d = DestructionProgress::default();
        d.set(1, [0, 0, 0], 3, 0);
        d.set(1, [0, 0, 0], 4, 300);
        d.tick(420);
        assert_eq!(d.breaker_count(), 1);
        d.tick(720);
        assert_eq!(d.breaker_count(), 0);
    }

    #[test]
    fn removing_an_unknown_breaker_is_inert() {
        let mut d = DestructionProgress::default();
        d.set(42, [0, 0, 0], 255, 0);
        assert_eq!(d.breaker_count(), 0);
        assert_eq!(d.position_count(), 0);
    }
}
