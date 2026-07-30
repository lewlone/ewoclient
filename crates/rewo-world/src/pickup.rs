//! `ItemPickupParticle` — the item that flies to whoever picked it up (M81).
//!
//! Vanilla implements this as a **particle carrying a captured entity render
//! state**, and the reason is the load-bearing one: by the time the animation
//! plays, the entity it draws is gone. `handleTakeItemEntity` removes it *in
//! the same handler* (see [`crate::pickup`] callers), so nothing on the entity
//! path could keep drawing it. The captured snapshot is the whole design.
//!
//! Three details read backwards:
//!
//! 1. **The velocity is dead.** The constructor passes the item's
//!    `getDeltaMovement()` into `Particle`'s `xd/yd/zd`, and then
//!    `ItemPickupParticle.tick` overrides without calling `super.tick()`, so
//!    nothing ever integrates it. The particle's own position is never used
//!    either — the render lerps from the *captured* position, not from `x/y/z`.
//! 2. **The source is frozen and the target is not.** `itemRenderState.{x,y,z}`
//!    is a snapshot at `partialTicks = 1.0`; `targetX/Y/Z` are re-read from the
//!    live collector every tick, with an `old` copy kept so the render can
//!    interpolate them. So the item chases a moving player.
//! 3. **The easing is quadratic in the *whole* life fraction**:
//!    `time = ((life + partial) / 3)²`. Not linear, and not eased at both ends
//!    — it starts slow and arrives fast.
//!
//! The target height is `(y + eyeY) / 2`, and `getEyeY()` is **absolute**
//! (`position.y + eyeHeight`), so the midpoint is the collector's chest — not
//! half an eye-height above the ground.

/// `ItemPickupParticle.LIFE_TIME`.
pub const LIFE_TIME: i32 = 3;

/// One in-flight pickup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pickup {
    /// The collected entity's captured `(item protocol id, count, hasFoil)`,
    /// or `None` when it had no stack to draw.
    ///
    /// `None` is not dead data: vanilla adds the particle for **anything** it
    /// was told was collected — an experience orb and an arrow included — and
    /// renders whatever that entity's renderer produces. Rewo has a model for
    /// the item case only, so the record exists and draws nothing. Keeping the
    /// record is what makes that a stated gap rather than a silent one.
    pub stack: Option<(i32, i32, bool)>,
    /// `itemRenderState.{x,y,z}` — the collected entity's position at capture,
    /// frozen for the whole flight.
    pub from: [f64; 3],
    /// The collector's entity id. Re-resolved every tick, because vanilla
    /// re-reads the live entity rather than snapshotting it.
    pub collector: i32,
    /// The collected entity's id, kept after its removal.
    ///
    /// It is the seed of everything else the renderer needs — `bobOffs` is
    /// derived from it and the per-copy jitter seed is derived from the stack
    /// — so one field carries the whole appearance forward rather than three
    /// copies that could disagree with the live path.
    pub entity_id: i32,
    /// `Particle.life`, 0..=[`LIFE_TIME`].
    pub life: i32,
    /// `targetXOld/YOld/ZOld`.
    pub target_prev: [f64; 3],
    /// `targetX/Y/Z`.
    pub target_cur: [f64; 3],
}

impl Pickup {
    /// Where to draw the item this frame.
    ///
    /// ```text
    /// float time = (life + partialTicks) / 3.0F;  time *= time;
    /// double xt = Mth.lerp(partialTicks, targetXOld, targetX);
    /// double xx = Mth.lerp(time, itemRenderState.x, xt);
    /// ```
    pub fn render_pos(&self, partial: f32) -> [f64; 3] {
        let p = partial.clamp(0.0, 1.0) as f64;
        let t = (self.life as f64 + p) / LIFE_TIME as f64;
        let t = t * t;
        let mut out = [0.0f64; 3];
        for i in 0..3 {
            let target = self.target_prev[i] + (self.target_cur[i] - self.target_prev[i]) * p;
            out[i] = self.from[i] + (target - self.from[i]) * t;
        }
        out
    }
}

/// Every in-flight pickup animation. Lives beside the entity table because it
/// is what carries an entity's *appearance* past its removal.
#[derive(Default, Debug)]
pub struct Pickups {
    live: Vec<Pickup>,
}

impl Pickups {
    /// `particleEngine.add(new ItemPickupParticle(...))`.
    ///
    /// `target` is the collector's chest, already resolved: the constructor
    /// runs `updatePosition()` then `saveOldPosition()`, so old and current
    /// start equal and the first frame's target interpolation is a no-op.
    pub fn add(
        &mut self,
        stack: Option<(i32, i32, bool)>,
        from: [f64; 3],
        collector: i32,
        entity_id: i32,
        target: [f64; 3],
    ) {
        self.live.push(Pickup {
            stack,
            from,
            collector,
            entity_id,
            life: 0,
            target_prev: target,
            target_cur: target,
        });
    }

    /// `ItemPickupParticle.tick` for every live animation.
    ///
    /// ```text
    /// this.life++;
    /// if (this.life == 3) this.remove();
    /// this.saveOldPosition();
    /// this.updatePosition();
    /// ```
    ///
    /// The removal is `== 3`, not `>= 3`, and it happens **before** the
    /// position bookkeeping — which is only observable in that a removed
    /// particle's last update is discarded with it.
    ///
    /// `resolve` returns the collector's chest position, or `None` when the
    /// collector is no longer resolvable; vanilla holds a hard reference to the
    /// entity, so the last known target is what a vanished collector leaves
    /// behind.
    pub fn tick(&mut self, mut resolve: impl FnMut(i32) -> Option<[f64; 3]>) {
        for p in &mut self.live {
            p.life += 1;
            p.target_prev = p.target_cur;
            if let Some(t) = resolve(p.collector) {
                p.target_cur = t;
            }
        }
        self.live.retain(|p| p.life < LIFE_TIME);
    }

    /// The live animations, for the renderer.
    pub fn iter(&self) -> impl Iterator<Item = &Pickup> {
        self.live.iter()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// A dimension change discards the level, and its particles with it.
    pub fn clear(&mut self) {
        self.live.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> Pickups {
        let mut p = Pickups::default();
        p.add(Some((1, 1, false)), [0.0, 0.0, 0.0], 5, 77, [0.0, 10.0, 0.0]);
        p
    }

    #[test]
    fn the_animation_lives_exactly_three_ticks() {
        let mut p = one();
        for _ in 0..2 {
            p.tick(|_| Some([0.0, 10.0, 0.0]));
            assert_eq!(p.len(), 1);
        }
        p.tick(|_| Some([0.0, 10.0, 0.0]));
        assert!(p.is_empty(), "life == LIFE_TIME removes");
    }

    #[test]
    fn the_flight_is_quadratic_not_linear() {
        let mut p = one();
        // The halfway point of a 3-tick life is `life 1, partial 0.5`, and it
        // has to be *reached* — sampling partial 0.5 at life 0 is a sixth of
        // the way through, not half.
        p.tick(|_| Some([0.0, 10.0, 0.0]));
        let a = p.iter().next().unwrap();
        // ((1 + 0.5) / 3)² = 0.25; the linear reading would be 0.5.
        let mid = a.render_pos(0.5);
        assert!(
            (mid[1] - 2.5).abs() < 1e-9,
            "half-way through, y = {} (linear would be 5.0)",
            mid[1]
        );
    }

    #[test]
    fn it_starts_at_the_captured_position_and_ends_at_the_target() {
        let mut p = one();
        assert_eq!(p.iter().next().unwrap().render_pos(0.0), [0.0, 0.0, 0.0]);
        p.tick(|_| Some([0.0, 10.0, 0.0]));
        p.tick(|_| Some([0.0, 10.0, 0.0]));
        // life 2, partial 1 → t = 1.0.
        let end = p.iter().next().unwrap().render_pos(1.0);
        assert!((end[1] - 10.0).abs() < 1e-9, "arrives at the collector: {end:?}");
    }

    #[test]
    fn the_target_tracks_a_moving_collector() {
        let mut p = one();
        p.tick(|_| Some([4.0, 10.0, 0.0]));
        let a = p.iter().next().unwrap();
        assert_eq!(a.target_prev, [0.0, 10.0, 0.0]);
        assert_eq!(a.target_cur, [4.0, 10.0, 0.0]);
        // Interpolating the target itself: partial 0.5 sits between them.
        // t = ((1 + 0.5)/3)² = 0.25, applied to x = 2.0 → 0.5.
        let mid = a.render_pos(0.5);
        assert!((mid[0] - 0.5).abs() < 1e-9, "x = {}", mid[0]);
    }

    #[test]
    fn a_vanished_collector_leaves_the_last_known_target() {
        let mut p = one();
        p.tick(|_| None);
        let a = p.iter().next().unwrap();
        assert_eq!(a.target_cur, [0.0, 10.0, 0.0]);
    }

    #[test]
    fn a_record_with_no_stack_still_exists() {
        // An experience orb: vanilla adds the particle regardless, and Rewo
        // has no orb model, so the record draws nothing.
        let mut p = Pickups::default();
        p.add(None, [0.0; 3], 1, 9, [0.0; 3]);
        assert_eq!(p.len(), 1);
        assert!(p.iter().next().unwrap().stack.is_none());
    }
}
