//! `ConduitBlockEntity.updateShape` — the prismarine-frame scan (M30).
//!
//! A conduit decides whether it is active **itself**, on the client, from the
//! blocks around it. The server sends nothing: no flag, no angle, no
//! activation packet. That is why the active cage sat unrendered until now —
//! it was never blocked on a clock, it was blocked on this scan.
//!
//! ```text
//! updateShape(level, pos, effectBlocks):
//!   for ox,oy,oz in -1..=1:                      // the 3x3x3 core
//!       if (!level.isWaterAt(pos + (ox,oy,oz))) return false;
//!   for ox,oy,oz in -2..=2:                      // the frame shell
//!       ax,ay,az = |ox|,|oy|,|oz|
//!       if ((ax > 1 || ay > 1 || az > 1)
//!           && (ox == 0 && (ay == 2 || az == 2)
//!            || oy == 0 && (ax == 2 || az == 2)
//!            || oz == 0 && (ax == 2 || ay == 2)))
//!           if (blockAt is one of VALID_BLOCKS) effectBlocks.add(...)
//!   return effectBlocks.size() >= 16;
//! ```
//!
//! Two thresholds come out of the same count, which is why `isHunting` is free
//! once the scan exists: `updateHunting` is simply
//! `setHunting(effectBlocks.size() >= 42)`.

/// `ConduitBlockEntity.VALID_BLOCKS` — what a frame may be built from.
///
/// Sea lantern counts; ordinary prismarine *slabs* and *stairs* do not, which
/// is why this is a list of exact block names rather than a "prismarine-ish"
/// prefix test.
pub const FRAME_BLOCKS: &[&str] = &[
    "minecraft:prismarine",
    "minecraft:prismarine_bricks",
    "minecraft:sea_lantern",
    "minecraft:dark_prismarine",
];

/// A conduit needs this many frame blocks to activate.
pub const ACTIVATE_AT: usize = 16;

/// ...and this many before it opens its eye.
///
/// **There are exactly 42 frame positions**, so this threshold means a
/// *complete* frame — not "nearly complete", which is what 42 looks like until
/// you count the shell. The two thresholds share one count, so the eye costs
/// nothing extra once the scan exists.
pub const HUNT_AT: usize = 42;

/// The offsets `updateShape` inspects in its second pass, in vanilla's
/// iteration order.
///
/// The condition picks the **three axis-aligned rings** of the ±2 shell: for
/// each axis, the ring lying in the plane where that axis is 0 and the other
/// two reach 2. Each ring is the border of a 5×5 plane — sixteen positions —
/// but the three rings **share their axis ends**, so the union is **42**, not
/// 48. That coincidence is load-bearing: [`HUNT_AT`] is also 42, so a conduit
/// opens its eye exactly when its frame is complete.
///
/// Precomputed rather than re-tested per scan, and named here so the gate can
/// count them.
pub fn frame_offsets() -> Vec<(i32, i32, i32)> {
    let mut out = Vec::with_capacity(48);
    for ox in -2i32..=2 {
        for oy in -2i32..=2 {
            for oz in -2i32..=2 {
                let (ax, ay, az) = (ox.abs(), oy.abs(), oz.abs());
                let outer = ax > 1 || ay > 1 || az > 1;
                let ring = (ox == 0 && (ay == 2 || az == 2))
                    || (oy == 0 && (ax == 2 || az == 2))
                    || (oz == 0 && (ax == 2 || ay == 2));
                if outer && ring {
                    out.push((ox, oy, oz));
                }
            }
        }
    }
    out
}

/// What one scan found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConduitShape {
    /// How many frame blocks were found. Zero when the water check failed,
    /// because vanilla returns before counting anything.
    pub frame: usize,
    /// Whether all 27 cells of the 3×3×3 core are water.
    pub submerged: bool,
}

impl ConduitShape {
    /// `updateShape`'s return — `effectBlocks.size() >= 16`.
    pub fn active(self) -> bool {
        self.submerged && self.frame >= ACTIVATE_AT
    }

    /// `updateHunting` — `effectBlocks.size() >= 42`.
    ///
    /// Note this reads the same count and is **not** gated on `active`
    /// separately: a shape with 42 frame blocks necessarily has 16.
    pub fn hunting(self) -> bool {
        self.submerged && self.frame >= HUNT_AT
    }
}

/// Run `updateShape` at a position.
///
/// `is_water` and `is_frame` are per-block-state predicates the caller
/// resolves once from the bake — the scan touches 27 + 48 cells and runs every
/// forty ticks per conduit, so neither should be a string comparison.
pub fn scan(
    at: (i32, i32, i32),
    mut state_at: impl FnMut(i32, i32, i32) -> u32,
    is_water: &[bool],
    is_frame: &[bool],
) -> ConduitShape {
    let (x, y, z) = at;
    let get = |s: &[bool], id: u32| s.get(id as usize).copied().unwrap_or(false);

    // The core must be water all the way round — vanilla returns early, so a
    // dry conduit reports a frame of zero rather than a count nobody looked at.
    for ox in -1..=1 {
        for oy in -1..=1 {
            for oz in -1..=1 {
                if !get(is_water, state_at(x + ox, y + oy, z + oz)) {
                    return ConduitShape {
                        frame: 0,
                        submerged: false,
                    };
                }
            }
        }
    }
    let mut frame = 0;
    for (ox, oy, oz) in frame_offsets() {
        if get(is_frame, state_at(x + ox, y + oy, z + oz)) {
            frame += 1;
        }
    }
    ConduitShape {
        frame,
        submerged: true,
    }
}

/// `ConduitBlockEntity`'s client-side animation state.
///
/// ```text
/// clientTick():  tickCount++;
///                if (gameTime % 40 == 0) { isActive = updateShape(...); updateHunting(...); }
///                if (isActive()) activeRotation++;
/// getActiveRotation(a) = (activeRotation + a) * -0.0375F
/// ```
///
/// Two details worth not normalising. The shape is re-scanned only **every
/// forty ticks**, so a conduit does not flicker while a player builds its
/// frame — it snaps on at the next multiple. And `activeRotation` advances
/// *only while active*, which is why a conduit that has never activated sits
/// at exactly zero and Rewo's dormant shell was already correct.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ConduitAnim {
    pub tick_count: i32,
    pub active_rotation: f32,
    pub shape: ConduitShape,
}

impl ConduitAnim {
    /// One client tick. `rescan` supplies a fresh shape on the ticks vanilla
    /// would run `updateShape`, and `None` on the rest.
    pub fn tick(&mut self, rescan: Option<ConduitShape>) {
        self.tick_count += 1;
        if let Some(s) = rescan {
            self.shape = s;
        }
        if self.shape.active() {
            self.active_rotation += 1.0;
        }
    }

    /// `getActiveRotation(partialTicks)`, in radians.
    ///
    /// The partial is added **only while active** (`isActive() ? partialTicks
    /// : 0`), so a conduit that has just switched off stops dead rather than
    /// creeping on through the frame.
    ///
    /// The `-0.0375` scale is vanilla's, and the sign is real: the cage turns
    /// the other way from what a bare tick count would give. The renderer then
    /// converts this to degrees and immediately back to radians, which is an
    /// exact round trip and therefore a no-op — worth knowing before
    /// "correcting" one half of it.
    pub fn rotation(self, alpha: f32) -> f32 {
        let a = if self.shape.active() { alpha } else { 0.0 };
        (self.active_rotation + a) * -0.0375
    }

    /// `state.animTime` — `tickCount + partialTicks`, which drives the cage's
    /// bob.
    pub fn anim_time(self, alpha: f32) -> f32 {
        self.tick_count as f32 + alpha
    }

    /// `state.animationPhase` — `tickCount / 66 % 3`.
    ///
    /// Integer division, so each phase holds for sixty-six ticks; the wind
    /// shroud swaps axis and texture between them.
    pub fn phase(self) -> i32 {
        (self.tick_count / 66) % 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_shell_is_forty_two_positions_not_forty_eight() {
        let offs = frame_offsets();
        assert_eq!(
            offs.len(),
            HUNT_AT,
            "three rings of sixteen sharing their axis ends — 42, not 48, and              exactly the hunting threshold"
        );
        // Every offset has exactly one zero component and reaches 2 in at
        // least one of the others — that is what "a ring" means here.
        for &(x, y, z) in &offs {
            let zeros = [x, y, z].iter().filter(|v: &&i32| **v == 0).count();
            assert!(zeros >= 1, "({x},{y},{z}) lies on no ring plane");
            assert!(x.abs().max(y.abs()).max(z.abs()) == 2, "({x},{y},{z})");
            // ...and no offset is inside the 3x3x3 core the water check covers.
            assert!(x.abs() > 1 || y.abs() > 1 || z.abs() > 1);
        }
    }

    #[test]
    fn a_dry_conduit_never_counts_its_frame() {
        // Vanilla returns BEFORE the counting loop, so a shape that fails the
        // water check reports zero rather than a partial count.
        let water = vec![false, true];
        let frame = vec![false, true];
        let shape = scan((0, 0, 0), |_, _, _| 0, &water, &frame);
        assert_eq!(shape, ConduitShape { frame: 0, submerged: false });
        assert!(!shape.active());
    }

    #[test]
    fn sixteen_frame_blocks_activate_and_forty_two_hunt() {
        let water = vec![false, true, true];
        let is_frame = vec![false, false, true];
        // State 1 is water-but-not-frame, state 2 is both.
        let n = std::cell::Cell::new(0usize);
        let offs = frame_offsets();
        let want: std::collections::HashSet<(i32, i32, i32)> =
            offs.iter().take(16).copied().collect();
        let _ = n;
        let s = scan(
            (0, 0, 0),
            |x, y, z| {
                if want.contains(&(x, y, z)) {
                    2
                } else {
                    1
                }
            },
            &water,
            &is_frame,
        );
        assert_eq!(s.frame, 16);
        assert!(s.active() && !s.hunting());

        // Every position — the hunting threshold IS the shell size.
        let want42: std::collections::HashSet<(i32, i32, i32)> =
            offs.iter().copied().collect();
        let s = scan(
            (0, 0, 0),
            |x, y, z| if want42.contains(&(x, y, z)) { 2 } else { 1 },
            &water,
            &is_frame,
        );
        assert_eq!(s.frame, 42);
        assert!(s.active() && s.hunting());
    }

    #[test]
    fn the_rotation_advances_only_while_active() {
        let mut a = ConduitAnim::default();
        let live = ConduitShape { frame: 20, submerged: true };
        let dead = ConduitShape { frame: 0, submerged: false };
        for _ in 0..10 {
            a.tick(Some(dead));
        }
        assert_eq!(a.active_rotation, 0.0, "a dormant conduit never turns");
        assert_eq!(a.rotation(0.5), 0.0, "and takes no partial either");
        for _ in 0..10 {
            a.tick(Some(live));
        }
        assert_eq!(a.active_rotation, 10.0);
        // (10 + 0.5) * -0.0375
        assert!((a.rotation(0.5) - (-0.39375)).abs() < 1e-6);
    }

    #[test]
    fn the_phase_holds_for_sixty_six_ticks_each() {
        let mut a = ConduitAnim::default();
        let live = ConduitShape { frame: 20, submerged: true };
        let mut seen = Vec::new();
        for _ in 0..200 {
            a.tick(Some(live));
            seen.push(a.phase());
        }
        assert_eq!(seen[0], 0);
        assert_eq!(seen[65], 1, "tick 66 is the first of phase 1");
        assert_eq!(seen[131], 2);
        assert_eq!(seen[197], 0, "and it wraps back round");
    }
}
