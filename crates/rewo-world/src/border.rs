//! The world border (M80) — `net.minecraft.world.level.border.WorldBorder`.
//!
//! Six packets write this one object, and everything else about the feature is
//! derived from it: the collision planes, the wall's colour, the red vignette,
//! and the wall's position at a partial tick.
//!
//! # The lerp's clock is **ticks**, and the `gameTime` it is handed is inert
//!
//! `lerpSizeBetween(from, to, ticks, gameTime)` looks like it starts a
//! wall-clock or game-time animation. It does not. `MovingBorderExtent` stores
//! `lerpProgress = duration` and `WorldBorder.tick()` — called once per client
//! tick from `ClientLevel.tick` — decrements it. The size at any moment is
//! `lerp((duration - progress) / duration, from, to)`, a pure function of a
//! **tick counter**.
//!
//! The `gameTime` argument is stored as `lerpBegin` / `lerpEnd` and is read by
//! exactly one method, `getLerpSpeed`, and there only as the difference
//! `lerpEnd - lerpBegin` — which is `duration` again. **No behaviour anywhere
//! depends on the game time the lerp began at**, so this port does not take it.
//! (The one genuinely non-tick clock in this feature is the *texture scroll* on
//! the rendered wall, which is `Util.getMillis() % 3000` — wall-clock
//! milliseconds. It lives in the renderer, not here.)
//!
//! # `getMinX()` is the **previous** tick's size, not this one's
//!
//! `getMinX()` delegates to `getMinX(0.0F)`, and `MovingBorderExtent` reads
//! `Mth.lerp(deltaPartialTick, previousSize, size)` — at partial 0 that is
//! `previousSize`. So during a lerp *every non-rendering consumer* (collision,
//! `isWithinBounds`, `getDistanceToBorder`, the HUD vignette) sees the size as
//! of the end of the **previous** tick, while the renderer — which passes a
//! real partial tick — sees this one. Making `min_x()` read the current size
//! looks like a fix and is a divergence.
//!
//! # `MAX_SIZE` is a `float` literal
//!
//! `public static final double MAX_SIZE = 5.999997E7F;` — the `F` suffix means
//! the value is rounded to `f32` *before* widening, so the double is exactly
//! **59999968.0**, not 59999970. Its half is exactly `absoluteMaxSize`
//! (29999984), so the default border sits precisely on the clamp bound.
//! Reading the literal as its decimal spelling gives a default border two
//! blocks wider that the clamp then silently corrects.

/// `BorderStatus`. Derived from the extent — never sent on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStatus {
    Growing,
    Shrinking,
    Stationary,
}

impl BorderStatus {
    /// `BorderStatus.getColor()`, the RGB the wall is tinted with.
    pub fn color(self) -> u32 {
        match self {
            // 4259712 == 0x40FF80, a green.
            BorderStatus::Growing => 0x0040_FF80,
            // 16724016 == 0xFF3030, a red.
            BorderStatus::Shrinking => 0x00FF_3030,
            // 2138367 == 0x20A0FF, a blue.
            BorderStatus::Stationary => 0x0020_A0FF,
        }
    }
}

/// `WorldBorder.MAX_SIZE`. See the module doc — the `F` suffix is load-bearing.
pub const MAX_SIZE: f64 = 5.999_997e7_f32 as f64;

/// The `absoluteMaxSize` field's initializer. A server overwrites it from
/// `initialize_border`; this is what the client uses before one arrives.
pub const DEFAULT_ABSOLUTE_MAX_SIZE: i32 = 29_999_984;

/// `Mth.clamp(double, double, double)` — `value < min ? min : Math.min(value, max)`.
///
/// Written out rather than calling `f64::clamp` because Rust's `f64::min`
/// returns the *other* operand when one side is NaN where Java's `Math.min`
/// propagates it, and `calculateSize` can produce a NaN from a zero duration.
fn mth_clamp(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// `Math.min(double, double)` — NaN-propagating, unlike `f64::min`.
fn java_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

/// `Math.max(double, double)` — NaN-propagating, unlike `f64::max`.
fn java_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else {
        b
    }
}

/// `Mth.lerp(alpha, p0, p1)` — `p0 + alpha * (p1 - p0)`.
fn mth_lerp(alpha: f64, p0: f64, p1: f64) -> f64 {
    p0 + alpha * (p1 - p0)
}

/// `WorldBorder.BorderExtent` — the two implementations, as one enum.
///
/// `StaticBorderExtent` caches its min/max box and refreshes it in
/// `onCenterChange` / `onAbsoluteMaxSizeChange`; `MovingBorderExtent` computes
/// on the fly and makes both hooks no-ops. Computing on the fly for both is
/// equivalent — the cache is invalidated on exactly the two events that change
/// its inputs — and removes a whole class of staleness bug.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Extent {
    Static {
        size: f64,
    },
    Moving {
        from: f64,
        to: f64,
        /// `lerpDuration`, a `double` in vanilla even though the argument is a
        /// `long` — the division that computes progress is a double divide.
        duration: f64,
        /// `lerpProgress`, a `long`, counting **down** from `duration`.
        progress: i64,
        size: f64,
        previous_size: f64,
    },
}

impl Extent {
    /// `MovingBorderExtent.calculateSize`.
    fn calculate_size(from: f64, to: f64, duration: f64, progress: i64) -> f64 {
        let p = (duration - progress as f64) / duration;
        // `progress < 1.0` is false for NaN (a zero duration), so a
        // zero-duration lerp reports `to` rather than a NaN size.
        if p < 1.0 {
            mth_lerp(p, from, to)
        } else {
            to
        }
    }

    fn size(&self) -> f64 {
        match *self {
            Extent::Static { size } => size,
            Extent::Moving { size, .. } => size,
        }
    }

    /// The size `getMinX(deltaPartialTick)` measures from.
    fn size_at(&self, partial: f32) -> f64 {
        match *self {
            Extent::Static { size } => size,
            Extent::Moving {
                size, previous_size, ..
            } => mth_lerp(partial as f64, previous_size, size),
        }
    }

    fn status(&self) -> BorderStatus {
        match *self {
            Extent::Static { .. } => BorderStatus::Stationary,
            // `to == from` never reaches here — `lerpSizeBetween` builds a
            // Static extent in that case — but vanilla's expression would call
            // it GROWING, so the `<` is transcribed rather than a `!=` guard.
            Extent::Moving { from, to, .. } => {
                if to < from {
                    BorderStatus::Shrinking
                } else {
                    BorderStatus::Growing
                }
            }
        }
    }

    fn lerp_target(&self) -> f64 {
        match *self {
            // `StaticBorderExtent.getLerpTarget` returns its own size.
            Extent::Static { size } => size,
            Extent::Moving { to, .. } => to,
        }
    }

    fn lerp_time(&self) -> i64 {
        match *self {
            Extent::Static { .. } => 0,
            Extent::Moving { progress, .. } => progress,
        }
    }

    /// `getLerpSpeed` — blocks of **diameter** per tick. Vanilla writes it as
    /// `abs(from - to) / (lerpEnd - lerpBegin)`, and that denominator is the
    /// duration: `lerpEnd` is `lerpBegin + duration`.
    fn lerp_speed(&self) -> f64 {
        match *self {
            Extent::Static { .. } => 0.0,
            Extent::Moving {
                from, to, duration, ..
            } => (from - to).abs() / duration,
        }
    }
}

/// `WorldBorder`, as the client holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBorder {
    center_x: f64,
    center_z: f64,
    absolute_max_size: i32,
    warning_blocks: i32,
    warning_time: i32,
    extent: Extent,
}

impl Default for WorldBorder {
    /// The field initializers of a freshly constructed `WorldBorder`, which is
    /// what `ClientLevel` gets before any packet arrives.
    ///
    /// **`warningTime = 15` is a stale pre-migration default.** 26.x moved the
    /// warning time from seconds to ticks — `WorldBorderWarningTimeFix`
    /// multiplies an old world's `warning_time` by 20, and `Settings.DEFAULT`
    /// duly says 300. This field initializer was left at the old *seconds*
    /// number and never fixed. It is invisible because every server overwrites
    /// it from `initialize_border` before the client renders anything.
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            absolute_max_size: DEFAULT_ABSOLUTE_MAX_SIZE,
            warning_blocks: 5,
            warning_time: 15,
            extent: Extent::Static { size: MAX_SIZE },
        }
    }
}

impl WorldBorder {
    pub fn center_x(&self) -> f64 {
        self.center_x
    }

    pub fn center_z(&self) -> f64 {
        self.center_z
    }

    pub fn warning_blocks(&self) -> i32 {
        self.warning_blocks
    }

    pub fn warning_time(&self) -> i32 {
        self.warning_time
    }

    pub fn absolute_max_size(&self) -> i32 {
        self.absolute_max_size
    }

    /// `setCenter`. Both extents keep reading `centerX` / `centerZ` live, so
    /// there is nothing to invalidate.
    pub fn set_center(&mut self, x: f64, z: f64) {
        self.center_x = x;
        self.center_z = z;
    }

    /// `setSize` — **replaces the extent wholesale**, so it cancels an
    /// in-flight lerp. `handleSetBorderSize` calls exactly this, which is why
    /// a `set_border_size` mid-lerp is a hard cut to the new diameter and not
    /// a new target for the running animation.
    pub fn set_size(&mut self, size: f64) {
        self.extent = Extent::Static { size };
    }

    /// `lerpSizeBetween`. `from == to` degrades to a static extent, which is
    /// the only reason `MovingBorderExtent.getStatus`'s `to < from` never has
    /// to consider equality.
    ///
    /// Vanilla's fourth argument, `gameTime`, is not taken — see the module
    /// doc. Nothing reads it.
    pub fn lerp_size_between(&mut self, from: f64, to: f64, ticks: i64) {
        self.extent = if from == to {
            Extent::Static { size: to }
        } else {
            let duration = ticks as f64;
            let size = Extent::calculate_size(from, to, duration, ticks);
            Extent::Moving {
                from,
                to,
                duration,
                progress: ticks,
                size,
                previous_size: size,
            }
        };
    }

    pub fn set_absolute_max_size(&mut self, size: i32) {
        self.absolute_max_size = size;
    }

    pub fn set_warning_blocks(&mut self, blocks: i32) {
        self.warning_blocks = blocks;
    }

    pub fn set_warning_time(&mut self, time: i32) {
        self.warning_time = time;
    }

    /// `WorldBorder.tick()` → `extent.update()`. One client tick.
    ///
    /// The order inside `MovingBorderExtent.update` is load-bearing:
    /// **decrement, then shift `previousSize`, then recompute `size`** — so
    /// after the call `previousSize` is the size the last tick ended on. The
    /// extent collapses to a static one the tick `lerpProgress` reaches zero.
    pub fn tick(&mut self) {
        if let Extent::Moving {
            from,
            to,
            duration,
            progress,
            size,
            previous_size,
        } = &mut self.extent
        {
            *progress -= 1;
            *previous_size = *size;
            *size = Extent::calculate_size(*from, *to, *duration, *progress);
            if *progress <= 0 {
                let to = *to;
                self.extent = Extent::Static { size: to };
            }
        }
    }

    /// `getSize` — the current tick's diameter.
    pub fn size(&self) -> f64 {
        self.extent.size()
    }

    /// `getLerpTarget`.
    pub fn lerp_target(&self) -> f64 {
        self.extent.lerp_target()
    }

    /// `getLerpTime` — the *remaining* ticks, not the original duration.
    pub fn lerp_time(&self) -> i64 {
        self.extent.lerp_time()
    }

    /// `getLerpSpeed` — diameter blocks per tick. The wall itself moves at
    /// half this, because the size is a diameter.
    pub fn lerp_speed(&self) -> f64 {
        self.extent.lerp_speed()
    }

    /// `getStatus`.
    pub fn status(&self) -> BorderStatus {
        self.extent.status()
    }

    pub fn min_x(&self, partial: f32) -> f64 {
        self.clamp_coord(self.center_x - self.extent.size_at(partial) / 2.0)
    }

    pub fn max_x(&self, partial: f32) -> f64 {
        self.clamp_coord(self.center_x + self.extent.size_at(partial) / 2.0)
    }

    pub fn min_z(&self, partial: f32) -> f64 {
        self.clamp_coord(self.center_z - self.extent.size_at(partial) / 2.0)
    }

    pub fn max_z(&self, partial: f32) -> f64 {
        self.clamp_coord(self.center_z + self.extent.size_at(partial) / 2.0)
    }

    fn clamp_coord(&self, v: f64) -> f64 {
        let m = self.absolute_max_size as f64;
        mth_clamp(v, -m, m)
    }

    /// `getDistanceToBorder(x, z)` — the smallest of the four wall distances,
    /// **negative outside**. Uses the partial-0 box, so during a lerp it
    /// measures against the previous tick's size (module doc).
    pub fn distance_to_border(&self, x: f64, z: f64) -> f64 {
        let from_north = z - self.min_z(0.0);
        let from_south = self.max_z(0.0) - z;
        let from_west = x - self.min_x(0.0);
        let from_east = self.max_x(0.0) - x;
        // Vanilla's exact fold order — it only shows through NaN, but it is
        // free to preserve.
        let m = java_min(from_west, from_east);
        let m = java_min(m, from_north);
        java_min(m, from_south)
    }

    /// `isWithinBounds(x, z, margin)`. Note the asymmetry vanilla wrote:
    /// `>=` on the minimum and a strict `<` on the maximum.
    pub fn is_within_bounds(&self, x: f64, z: f64, margin: f64) -> bool {
        x >= self.min_x(0.0) - margin
            && x < self.max_x(0.0) + margin
            && z >= self.min_z(0.0) - margin
            && z < self.max_z(0.0) + margin
    }

    /// The distance at which the red vignette starts, from `Hud.extractVignette`.
    ///
    /// **The warning *delay* and the warning *distance* collapse into one
    /// number here.** `warningTime` is in **ticks** (26.x's `TimeArgument`
    /// parses a bare number as ticks) and `getLerpSpeed` is in blocks per
    /// tick, so their product is a distance: how far the wall will travel in
    /// the warning window. That is capped by the total travel still to come
    /// (`|lerpTarget - size|`, so a nearly-finished shrink stops warning), and
    /// the flat `warningBlocks` is a floor under the result.
    ///
    /// The two terms of that `min` read from different clocks on purpose:
    /// `lerp_speed` is a property of the whole animation while `size` is this
    /// tick's.
    pub fn warning_distance(&self) -> f64 {
        let moving_blocks_threshold = java_min(
            self.lerp_speed() * self.warning_time as f64,
            (self.lerp_target() - self.size()).abs(),
        );
        java_max(self.warning_blocks as f64, moving_blocks_threshold)
    }

    /// The vignette's `borderWarningStrength` in `0..=1`, or 0 for no warning.
    ///
    /// The `as f32` on the distance is vanilla's: `Hud` narrows
    /// `getDistanceToBorder` to a `float` before comparing, then widens again
    /// for the divide.
    pub fn warning_strength(&self, x: f64, z: f64) -> f32 {
        let dist_to_border = self.distance_to_border(x, z) as f32;
        let warning_distance = self.warning_distance();
        if (dist_to_border as f64) < warning_distance {
            let s = 1.0f32 - (dist_to_border as f64 / warning_distance) as f32;
            s.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The border's collision extent, floored/ceiled the way
    /// `getCollisionShape` does. Always available; whether it *applies* to a
    /// given entity is [`BorderCollision::is_inside_close_to_border`].
    pub fn collision(&self) -> BorderCollision {
        BorderCollision {
            min_x: self.min_x(0.0),
            max_x: self.max_x(0.0),
            min_z: self.min_z(0.0),
            max_z: self.max_z(0.0),
        }
    }

    /// One frame's render inputs, `WorldBorderRenderer.extract`.
    ///
    /// `render_distance` is `options.getEffectiveRenderDistance() * 16`, in
    /// blocks. Returns `None` when the wall is invisible — which is both the
    /// "deep inside the border" case and the "far outside it" case.
    pub fn extract(
        &self,
        partial: f32,
        camera_x: f64,
        camera_z: f64,
        render_distance: f64,
    ) -> Option<BorderRender> {
        let min_x = self.min_x(partial);
        let max_x = self.max_x(partial);
        let min_z = self.min_z(partial);
        let max_z = self.max_z(partial);
        // Vanilla's condition, de-Morganed into two readable halves. The first
        // group is "the camera is NOT deep inside" — it is within
        // `renderDistance` of at least one wall, or outside altogether. The
        // second is "not more than `renderDistance` outside".
        let near_a_wall = !(camera_x < max_x - render_distance)
            || !(camera_x > min_x + render_distance)
            || !(camera_z < max_z - render_distance)
            || !(camera_z > min_z + render_distance);
        let not_far_outside = !(camera_x < min_x - render_distance)
            && !(camera_x > max_x + render_distance)
            && !(camera_z < min_z - render_distance)
            && !(camera_z > max_z + render_distance);
        if !(near_a_wall && not_far_outside) {
            return None;
        }
        // `1 - d/rd`, then **pow 4, then clamp** — that order is vanilla's, and
        // it matters outside the border where `1 - d/rd` exceeds 1 and the
        // fourth power amplifies before the clamp catches it.
        let alpha = 1.0 - self.distance_to_border(camera_x, camera_z) / render_distance;
        let alpha = alpha.powf(4.0);
        let alpha = mth_clamp(alpha, 0.0, 1.0);
        Some(BorderRender {
            min_x,
            max_x,
            min_z,
            max_z,
            tint: self.status().color(),
            alpha,
        })
    }
}

/// The world border as a collider — `WorldBorder.getCollisionShape()`.
///
/// The shape is the **complement** of a box: `Shapes.join(INFINITY, box(...),
/// ONLY_FIRST)`, infinite in Y, so being "inside" the shape means being outside
/// the border. The box's horizontal bounds are `floor`ed and `ceil`ed, so the
/// wall you *collide* with is snapped outward to whole blocks while the wall
/// you *see* is at the exact fractional coordinate. For an integer-sized,
/// integer-centred border they coincide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderCollision {
    /// The exact (unfloored) bounds — what the `isInsideCloseToBorder` gate
    /// measures against.
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl BorderCollision {
    pub fn plane_min_x(&self) -> f64 {
        self.min_x.floor()
    }

    pub fn plane_max_x(&self) -> f64 {
        self.max_x.ceil()
    }

    pub fn plane_min_z(&self) -> f64 {
        self.min_z.floor()
    }

    pub fn plane_max_z(&self) -> f64 {
        self.max_z.ceil()
    }

    fn distance_to_border(&self, x: f64, z: f64) -> f64 {
        let m = java_min(x - self.min_x, self.max_x - x);
        let m = java_min(m, z - self.min_z);
        java_min(m, self.max_z - z)
    }

    fn is_within_bounds(&self, x: f64, z: f64, margin: f64) -> bool {
        x >= self.min_x - margin
            && x < self.max_x + margin
            && z >= self.min_z - margin
            && z < self.max_z + margin
    }

    /// `WorldBorder.isInsideCloseToBorder(entity, boundingBox)` — the gate
    /// `collectCollidersIgnoringWorldBorder` gives the border shape.
    ///
    /// **Without it, anything outside the border would be sealed in solid**:
    /// the shape is an infinite complement, so an entity that has been
    /// teleported past the wall is inside it everywhere. The gate switches the
    /// collider off once you are further than roughly your own width outside,
    /// which is what lets you walk back.
    ///
    /// The two arguments come from different places, and the split is easy to
    /// miss: `bbMax` is derived from the **movement-expanded** box, while the
    /// distance is measured from the **entity's own x/z**.
    pub fn is_inside_close_to_border(
        &self,
        entity_x: f64,
        entity_z: f64,
        box_x_size: f64,
        box_z_size: f64,
    ) -> bool {
        // `Mth.absMax(a, b)` is `max(|a|, |b|)`.
        let bb_max = java_max(java_max(box_x_size.abs(), box_z_size.abs()), 1.0);
        self.distance_to_border(entity_x, entity_z) < bb_max * 2.0
            && self.is_within_bounds(entity_x, entity_z, bb_max)
    }
}

/// One frame of the wall, ready for the renderer. Plain numbers so `rewo-gpu`
/// needs no dependency on this crate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderRender {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
    /// `BorderStatus.getColor()`, 0x00RRGGBB.
    pub tint: u32,
    pub alpha: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_size_is_the_float_literal_widened_not_its_decimal_spelling() {
        // 5.999997E7F rounds to 59999968 as an f32; the decimal reading would
        // be 59999970. The difference is what puts `MAX_SIZE / 2` exactly on
        // the `absoluteMaxSize` clamp bound instead of one past it.
        assert_eq!(MAX_SIZE, 59_999_968.0);
        assert_eq!(MAX_SIZE / 2.0, DEFAULT_ABSOLUTE_MAX_SIZE as f64);
    }

    #[test]
    fn a_fresh_border_is_stationary_at_the_clamp_bound() {
        let b = WorldBorder::default();
        assert_eq!(b.status(), BorderStatus::Stationary);
        assert_eq!(b.min_x(0.0), -29_999_984.0);
        assert_eq!(b.max_x(0.0), 29_999_984.0);
        assert_eq!(b.warning_time(), 15);
        assert_eq!(b.warning_blocks(), 5);
    }

    #[test]
    fn a_lerp_starts_at_from_and_walks_one_step_per_tick() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(100.0, 200.0, 10);
        assert_eq!(b.size(), 100.0);
        assert_eq!(b.status(), BorderStatus::Growing);
        assert_eq!(b.lerp_time(), 10);
        for i in 1..=9 {
            b.tick();
            assert_eq!(b.size(), 100.0 + 10.0 * i as f64, "after {i} ticks");
        }
        // The tenth tick lands on the target and collapses to static.
        b.tick();
        assert_eq!(b.size(), 200.0);
        assert_eq!(b.status(), BorderStatus::Stationary);
        assert_eq!(b.lerp_time(), 0);
    }

    #[test]
    fn set_size_cancels_an_in_flight_lerp() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(100.0, 200.0, 100);
        b.tick();
        b.set_size(64.0);
        assert_eq!(b.size(), 64.0);
        assert_eq!(b.status(), BorderStatus::Stationary);
        b.tick();
        assert_eq!(b.size(), 64.0, "a cancelled lerp does not resume");
    }

    #[test]
    fn an_equal_from_and_to_never_builds_a_moving_extent() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(50.0, 50.0, 100);
        assert_eq!(b.status(), BorderStatus::Stationary);
        assert_eq!(b.lerp_speed(), 0.0);
    }

    #[test]
    fn partial_zero_reads_the_previous_tick_while_partial_one_reads_this_one() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(100.0, 200.0, 10);
        b.tick(); // size 110, previous 100
        assert_eq!(b.min_x(0.0), -50.0, "partial 0 is the previous size");
        assert_eq!(b.min_x(1.0), -55.0, "partial 1 is this tick's size");
        assert_eq!(b.size(), 110.0);
    }

    #[test]
    fn shrinking_and_growing_come_from_the_direction_not_the_wire() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(200.0, 100.0, 10);
        assert_eq!(b.status(), BorderStatus::Shrinking);
        assert_eq!(b.status().color(), 0x00FF_3030);
        b.lerp_size_between(100.0, 200.0, 10);
        assert_eq!(b.status().color(), 0x0040_FF80);
        b.set_size(1.0);
        assert_eq!(b.status().color(), 0x0020_A0FF);
    }

    #[test]
    fn a_zero_duration_lerp_reports_the_target_rather_than_a_nan() {
        // `set_border_lerp_size` has no `lerpTime > 0` guard, so this is
        // reachable from the wire.
        let mut b = WorldBorder::default();
        b.lerp_size_between(100.0, 200.0, 0);
        assert_eq!(b.size(), 200.0, "NaN progress falls through to `to`");
        assert!(b.lerp_speed().is_infinite());
        b.tick();
        assert_eq!(b.status(), BorderStatus::Stationary);
    }

    #[test]
    fn the_distance_is_negative_outside() {
        let mut b = WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(20.0); // ±10
        assert_eq!(b.distance_to_border(0.0, 0.0), 10.0);
        assert_eq!(b.distance_to_border(9.0, 0.0), 1.0);
        assert_eq!(b.distance_to_border(12.0, 0.0), -2.0);
    }

    #[test]
    fn the_warning_distance_is_the_max_of_the_flat_blocks_and_the_travel() {
        let mut b = WorldBorder::default();
        b.set_warning_blocks(5);
        b.set_warning_time(100);
        // Stationary: lerp speed 0, so only the flat blocks count.
        b.set_size(20.0);
        assert_eq!(b.warning_distance(), 5.0);
        // Shrinking 200 → 100 over 100 ticks: 1 block of diameter per tick,
        // × 100 ticks of warning = 100, capped by the 100 blocks still to
        // travel → 100, which beats the flat 5.
        b.lerp_size_between(200.0, 100.0, 100);
        assert_eq!(b.warning_distance(), 100.0);
        // Once only 10 blocks of travel remain the cap binds instead.
        for _ in 0..90 {
            b.tick();
        }
        assert_eq!(b.size(), 110.0);
        assert_eq!(b.warning_distance(), 10.0);
    }

    #[test]
    fn the_warning_strength_ramps_to_one_at_the_wall() {
        let mut b = WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(20.0);
        b.set_warning_blocks(5);
        assert_eq!(b.warning_strength(0.0, 0.0), 0.0, "10 away, no warning");
        // `1.0F - (float)(4.0/5.0)` is 0.19999999, not 0.2 — the subtraction
        // happens in `float`, as `Hud` writes it. Computing the whole thing in
        // `f64` and narrowing at the end would give exactly 0.2 and be wrong.
        assert_eq!(b.warning_strength(6.0, 0.0), 1.0f32 - 0.8f32, "4 away of 5");
        assert!((b.warning_strength(6.0, 0.0) - 0.2).abs() < 1e-7);
        assert_eq!(b.warning_strength(10.0, 0.0), 1.0, "at the wall");
        assert_eq!(b.warning_strength(12.0, 0.0), 1.0, "outside, clamped");
    }

    #[test]
    fn the_collision_planes_snap_outward_to_whole_blocks() {
        let mut b = WorldBorder::default();
        b.set_center(0.5, 0.5);
        b.set_size(9.0); // x from -4.0 to 5.0, z the same
        let c = b.collision();
        assert_eq!(c.min_x, -4.0);
        assert_eq!(c.max_x, 5.0);
        b.set_size(9.5); // x from -4.25 to 5.25
        let c = b.collision();
        assert_eq!(c.min_x, -4.25, "the exact bound is kept for the gate");
        assert_eq!(c.plane_min_x(), -5.0, "the collider floors outward");
        assert_eq!(c.plane_max_x(), 6.0, "and ceils outward");
    }

    #[test]
    fn the_collider_switches_off_once_you_are_well_outside() {
        let mut b = WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(20.0);
        let c = b.collision();
        // A player: 0.6 wide, so `bbMax` is the 1.0 floor.
        assert!(
            c.is_inside_close_to_border(9.0, 0.0, 0.6, 0.6),
            "1 block inside the wall: the collider applies"
        );
        assert!(
            !c.is_inside_close_to_border(0.0, 0.0, 0.6, 0.6),
            "10 blocks in: too far from any wall for the shape to matter"
        );
        assert!(
            c.is_inside_close_to_border(10.5, 0.0, 0.6, 0.6),
            "just outside: still gated in, so you are pushed back"
        );
        assert!(
            !c.is_inside_close_to_border(12.0, 0.0, 0.6, 0.6),
            "well outside: the collider is withheld, or you would be sealed in"
        );
    }

    #[test]
    fn the_wall_is_invisible_deep_inside_and_far_outside() {
        let mut b = WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(2000.0); // ±1000
        assert!(
            b.extract(1.0, 0.0, 0.0, 160.0).is_none(),
            "1000 blocks from every wall with a 160-block view"
        );
        let r = b.extract(1.0, 900.0, 0.0, 160.0).expect("100 from the wall");
        assert!(r.alpha > 0.0 && r.alpha < 1.0);
        assert_eq!(r.tint, BorderStatus::Stationary.color());
        assert!(
            b.extract(1.0, 1200.0, 0.0, 160.0).is_none(),
            "200 blocks outside a 160-block view"
        );
        let r = b.extract(1.0, 1000.0, 0.0, 160.0).expect("at the wall");
        assert_eq!(r.alpha, 1.0, "distance 0 → alpha 1");
    }

    #[test]
    fn the_alpha_curve_is_the_fourth_power() {
        let mut b = WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(2000.0);
        // 80 blocks from the wall with a 160-block view: 1 - 80/160 = 0.5,
        // and 0.5^4 = 0.0625. A linear ramp would say 0.5.
        let r = b.extract(1.0, 920.0, 0.0, 160.0).unwrap();
        assert!((r.alpha - 0.0625).abs() < 1e-12, "alpha {}", r.alpha);
    }
}
