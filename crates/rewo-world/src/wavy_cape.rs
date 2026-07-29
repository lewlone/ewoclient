//! The wavy (cloth-simulated) cape — M61.
//!
//! **This module has no vanilla oracle.** Vanilla's cape is one rigid slab;
//! a cape that behaves like cloth is a mod convention, and the mod that
//! popularised it is licensed such that its source may not be read
//! (`REWO_FEATURE_SURVEY.md` §2). Everything here is derived from
//! [`REWO_WAVY_CAPE_SPEC.md`](../../../REWO_WAVY_CAPE_SPEC.md), which was
//! written first and is the source of truth, plus textbook constrained-
//! particle cloth: Verlet integration (Verlet 1967), mass–spring cloth
//! (Provot 1995), position-based distance constraints (Jakobsen 2001).
//!
//! **The numbers below are chosen, not derived**, and they are the spec's.
//! Do not tune them here — change the spec first.
//!
//! # What is simulated, and in which space
//!
//! The cape becomes `SEGMENTS` stacked slabs and therefore `SEGMENTS + 1`
//! joints on its spine. Joint 0 is **pinned** to the vanilla attachment
//! point; every joint below it simulates.
//!
//! The chain lives in **cape space**: world-axis-aligned (so +Y is world
//! up), measured in **model units** (1 block = 16), with its origin on the
//! entity. Two consequences make this the only workable frame:
//!
//! * It **translates with the player but does not rotate with them**, so a
//!   turn swings the anchor around the body and the chain has to catch up.
//!   In a body-attached frame a pure yaw would rotate the whole chain
//!   rigidly and there would be no wave at all.
//! * The torso push-out is then a cylinder about the cape-space Y axis,
//!   needing no knowledge of which way the player faces.
//!
//! The **absolute Y offset of cape space is deliberately unspecified** —
//! gravity is uniform, the torso cylinder is infinite in Y, and the
//! divergence clamp is anchor-relative, so nothing here can observe it. The
//! renderer re-pins joint 0 onto the true attachment point (which it derives
//! from the animated body transform, the clearance shift and the death roll,
//! none of which this module can see) and rigidly translates the rest.
//!
//! # Reading the spec's two under-determined numbers
//!
//! Both are recorded here because a later reader will otherwise re-derive
//! them from scratch — and because they are the two places this module could
//! be wrong without any witness noticing.
//!
//! 1. **`GRAVITY` acts along world down.** The spec says "model units per
//!    tick², downward"; "downward" is read as the world's down, not the
//!    cape's own local down. The alternative — gravity along the vanilla
//!    cape's current spine direction — would make the settled state
//!    *exactly* the vanilla drape (including its 6° rest tilt) and would be
//!    robust to point 2 below, but it destroys the reduction rule's stated
//!    mutation: a single simulated segment would then settle onto the
//!    vanilla angle instead of hanging straight down, and the reduction
//!    witness is the milestone's whole safety net.
//! 2. **The anchor delta is used verbatim, in blocks.** The spec says the
//!    acceleration is "gravity plus the lagging-anchor delta vector
//!    `(deltaX, deltaY, deltaZ)` the vanilla cape already computes" and
//!    gives no conversion. Converting it to model units (×16) is the
//!    dimensionally coherent reading and is what a first pass would write —
//!    it is also unusable: a sprinting player's delta is ≈0.9 blocks, so
//!    ×16 is ≈14 model units per tick², fourteen link-lengths in one step.
//!    The chain then snaps rigidly onto the acceleration direction in a
//!    single tick and never waves. Verbatim blocks give ≈0.9, comparable to
//!    `REST_LEN`, which is the regime a position-based solver produces
//!    cloth in. See the milestone report: this is the number most likely to
//!    want a coefficient.

/// Slabs the cape is divided into.
///
/// 16 is enough to read as cloth, and `16/16` makes [`REST_LEN`] exactly
/// `1.0` — an exactly-representable binary fraction, which the
/// bit-determinism witness wants and `16/12` would not have given.
pub const SEGMENTS: usize = 16;

/// Model units per tick², downward. See the module header for what
/// "downward" is read as.
pub const GRAVITY: f64 = 0.008;

/// Velocity retained per tick.
pub const DAMPING: f64 = 0.92;

/// Gauss–Seidel distance-constraint iterations.
pub const RELAX_PASSES: usize = 4;

/// Equal to the slab height, so the rest state is straight.
pub const REST_LEN: f64 = 16.0 / SEGMENTS as f64;

/// Push-out cylinder about the body's vertical axis, in model units.
///
/// Not an arbitrary radius: the vanilla cape's spine leaves its pivot at
/// model z ≈ 2.497 and only moves further back, so the cylinder grazes the
/// rest pose and bites only when the chain swings inward.
pub const TORSO_RADIUS: f64 = 2.5;

/// Divergence backstop. A joint further than this from the anchor is clamped
/// back to it.
///
/// **Unreachable while the constraints hold** — the chain's total length is
/// `REST_LEN * SEGMENTS` = 16, so no joint can exceed 16 from the anchor
/// unless the solver has already failed. [`WavyCape::relax`] is the only
/// thing that can fail, and it fails exactly one way: by declining a link
/// whose length is not finite and positive. That is what the `capeshot`
/// backstop witness constructs.
pub const MAX_JOINT_RADIUS: f64 = 24.0;

/// One player's simulated cape spine.
///
/// `cur` and `prev` are the Verlet position pair — `prev` is the previous
/// **tick's** post-constraint state, which is exactly what the render
/// interpolates against. A frame must never call [`Self::tick`].
#[derive(Clone, Debug, PartialEq)]
pub struct WavyCape {
    cur: Vec<[f64; 3]>,
    prev: Vec<[f64; 3]>,
}

impl WavyCape {
    /// A chain of `segments` slabs (so `segments + 1` joints), hanging
    /// straight down from `anchor` — which is its own settled state under
    /// world-down gravity, so a cape that spawns in view does not visibly
    /// fall into place.
    pub fn new(segments: usize, anchor: [f64; 3]) -> Self {
        let n = segments.max(1) + 1;
        let cur: Vec<[f64; 3]> = (0..n)
            .map(|j| [anchor[0], anchor[1] - j as f64 * REST_LEN, anchor[2]])
            .collect();
        Self {
            prev: cur.clone(),
            cur,
        }
    }

    pub fn segments(&self) -> usize {
        self.cur.len() - 1
    }

    /// The current (end-of-tick) joint positions, in cape space.
    pub fn joints(&self) -> &[[f64; 3]] {
        &self.cur
    }

    /// The previous tick's joint positions — the other end of the render
    /// interpolation.
    pub fn prev_joints(&self) -> &[[f64; 3]] {
        &self.prev
    }

    /// Advance one 20 Hz tick.
    ///
    /// * `anchor` — where joint 0 is pinned this tick, in cape space.
    /// * `delta` — vanilla's lagging-cloak gap `cloak - pos`, in **blocks**
    ///   and in world axes. See the module header for why it is not scaled.
    ///
    /// The order is the spec's and is fixed: Verlet, then `RELAX_PASSES`
    /// relaxation passes, then the torso push-out, then the radius clamp.
    ///
    /// The four stages are public individually **only** so `capeshot` can
    /// observe the constraint residual where the spec names it — after the
    /// relax passes and before the push-out perturbs them again. This is the
    /// only correct order and is what everything in production calls.
    pub fn tick(&mut self, anchor: [f64; 3], delta: [f64; 3]) {
        self.integrate(anchor, delta);
        self.relax();
        self.push_out();
        self.clamp(anchor);
    }

    /// Stage 1: Verlet, then pin joint 0.
    pub fn integrate(&mut self, anchor: [f64; 3], delta: [f64; 3]) {
        // `dt` is one tick, so `a·dt²` is `a` and GRAVITY is already
        // per-tick². Writing the multiply out would only invite someone to
        // "fix" it with a seconds-based dt.
        let a = [delta[0], delta[1] - GRAVITY, delta[2]];
        // Swap first, so no buffer is allocated and `prev` ends up holding
        // this tick's starting state without a copy: afterwards `self.prev`
        // is the old `cur` (what we must keep) and `self.cur` is the old
        // `prev` (scratch the integrator overwrites).
        std::mem::swap(&mut self.cur, &mut self.prev);
        for j in 1..self.cur.len() {
            for k in 0..3 {
                let c = self.prev[j][k];
                let p = self.cur[j][k];
                self.cur[j][k] = c + (c - p) * DAMPING + a[k];
            }
        }
        // Joint 0 does not simulate. It is *assigned*, every tick, which is
        // what the pinning witness asserts and what makes the anchor's own
        // motion the chain's only driver besides gravity.
        self.cur[0] = anchor;
    }

    /// The worst `|link - REST_LEN|` in the chain right now.
    ///
    /// Read between [`Self::relax`] and [`Self::push_out`] this is the
    /// spec's constraint residual; read after a whole [`Self::tick`] it also
    /// carries whatever the push-out just did, which is a different and
    /// larger number — the collision response is not re-projected.
    pub fn worst_link_error(&self) -> f64 {
        (0..self.segments())
            .map(|i| {
                let (p, q) = (self.cur[i], self.cur[i + 1]);
                let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() - REST_LEN).abs()
            })
            .fold(0.0, f64::max)
    }

    /// Sequential (Gauss–Seidel) distance-constraint projection, from the
    /// pinned end outward.
    ///
    /// # Why only the lower joint moves
    ///
    /// The mass weighting is `w_upper = 0, w_lower = 1`: walking the links
    /// from the pin, each link's upper joint has already been placed by the
    /// link above it and is treated as fixed. For a chain pinned at one end
    /// this makes the projection **exact** — every link lands at `REST_LEN`
    /// to float precision, in one pass — which is the only way to meet the
    /// spec's "every link within 1e-4 of `REST_LEN` after the relax passes".
    /// The symmetric weighting a cloth sheet needs leaves ≈3e-2 of residual
    /// after four passes on this chain (measured), because gravity's uniform
    /// per-tick shift breaks link 0 against the pin every tick and four
    /// Gauss–Seidel sweeps only halve that error four times.
    ///
    /// Passes 2..`RELAX_PASSES` are therefore exact no-ops. They are still
    /// run: `RELAX_PASSES` is a spec constant, and a future weighting change
    /// would silently lose them.
    ///
    /// A link whose length is not finite and positive is **left alone**
    /// rather than given an invented direction. That is the solver failing,
    /// and failing loudly enough for [`Self::clamp`] to see it — see
    /// [`MAX_JOINT_RADIUS`].
    pub fn relax(&mut self) {
        for _ in 0..RELAX_PASSES {
            for i in 0..self.cur.len() - 1 {
                let p = self.cur[i];
                let q = self.cur[i + 1];
                let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if !len.is_finite() || len <= 0.0 {
                    continue;
                }
                let s = REST_LEN / len;
                self.cur[i + 1] = [p[0] + d[0] * s, p[1] + d[1] * s, p[2] + d[2] * s];
            }
        }
    }

    /// Push any joint inside [`TORSO_RADIUS`] of the body's vertical axis
    /// radially out to it.
    ///
    /// This is the one place a naive chain visibly fails: without it the
    /// cape passes through the player on a fast turn. Measured on the
    /// prototype, a scripted 180° turn takes a joint to radius 0.46 — well
    /// inside the torso — with this disabled.
    ///
    /// Joint 0 is exempt: it is pinned, and the vanilla attachment point is
    /// itself a hair inside the cylinder (radius ≈2.497).
    pub fn push_out(&mut self) {
        for j in 1..self.cur.len() {
            let p = self.cur[j];
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            if !r.is_finite() {
                // Left for `clamp`; scaling a non-finite radius here would
                // only launder it into a plausible-looking finite position.
                continue;
            }
            if r < TORSO_RADIUS {
                if r > 0.0 {
                    let s = TORSO_RADIUS / r;
                    self.cur[j] = [p[0] * s, p[1], p[2] * s];
                } else {
                    // Exactly on the axis: any direction is as good as any
                    // other, so pick one deterministically.
                    self.cur[j] = [p[0], p[1], TORSO_RADIUS];
                }
            }
        }
    }

    /// The divergence backstop.
    ///
    /// The normalization deliberately does **not** match [`Self::relax`]'s:
    /// it divides through by the largest component first, so an offset of
    /// 1e200 — whose squared length overflows to infinity and which the
    /// relax therefore declines — still yields a usable direction here. The
    /// asymmetry is the point: the constraints give up, and the clamp is
    /// what recovers.
    pub fn clamp(&mut self, anchor: [f64; 3]) {
        for j in 1..self.cur.len() {
            let off = [
                self.cur[j][0] - anchor[0],
                self.cur[j][1] - anchor[1],
                self.cur[j][2] - anchor[2],
            ];
            let len = (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt();
            // `!(len <= MAX)` and not `len > MAX`, so a NaN is caught too.
            if len <= MAX_JOINT_RADIUS {
                continue;
            }
            let dir = overflow_safe_dir(off).unwrap_or([0.0, -1.0, 0.0]);
            self.cur[j] = [
                anchor[0] + dir[0] * MAX_JOINT_RADIUS,
                anchor[1] + dir[1] * MAX_JOINT_RADIUS,
                anchor[2] + dir[2] * MAX_JOINT_RADIUS,
            ];
        }
    }

    /// Render interpolation: joint positions `alpha` of the way from the
    /// previous tick to the current one.
    ///
    /// Takes `&self`. Frames interpolate; they never advance the simulation.
    pub fn interpolated(&self, alpha: f32, out: &mut [[f32; 3]]) -> usize {
        let a = alpha.clamp(0.0, 1.0) as f64;
        let n = self.cur.len().min(out.len());
        for j in 0..n {
            for k in 0..3 {
                out[j][k] = (self.prev[j][k] + (self.cur[j][k] - self.prev[j][k]) * a) as f32;
            }
        }
        n
    }
}

/// Normalize a vector whose squared length may overflow, by scaling out its
/// largest component first. `None` for a zero or non-finite input.
fn overflow_safe_dir(v: [f64; 3]) -> Option<[f64; 3]> {
    let m = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if !m.is_finite() || m <= 0.0 {
        return None;
    }
    let s = [v[0] / m, v[1] / m, v[2] / m];
    let len = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
    if !len.is_finite() || len <= 0.0 {
        return None;
    }
    Some([s[0] / len, s[1] / len, s[2] / len])
}

/// `PlayerCapeModel.setupAnim`'s net cape rotation, in f64.
///
/// A second derivation of `rewo_gpu::entities::cape_rotation` — the renderer
/// cannot be reached from here (rewo-gpu is a leaf crate that depends on no
/// other rewo crate), and the simulation needs the same matrix to find its
/// anchor. `capeshot` grades the two against each other rather than letting
/// the duplication sit unchecked; see the `h`-series witnesses.
fn cape_rotation(flap_deg: f64, lean_deg: f64, lean2_deg: f64) -> [[f64; 3]; 3] {
    let a = (6.0 + lean_deg / 2.0 + flap_deg).to_radians();
    let b = (lean2_deg / 2.0).to_radians();
    let c = (180.0 - lean2_deg / 2.0).to_radians();
    mul(mul(rot_x(a), rot_z(b)), rot_y(c))
}

fn rot_x(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rot_z(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

/// The cape's spine attachment point, in **model space**.
///
/// `PlayerCapeModel`'s cube spans x −5..5, y 0..16, z −1..0, so its spine —
/// the line the joints sit on — is at local (0, y, −0.5). Joint 0 is that
/// line's top, rotated by the cape's net rotation and offset by the
/// `PartPose` pivot.
pub fn anchor_model(flap: f32, lean: f32, lean2: f32) -> [f64; 3] {
    let r = cape_rotation(flap as f64, lean as f64, lean2 as f64);
    // r · (0, 0, −0.5), i.e. −0.5 × the third column.
    [
        -0.5 * r[0][2] + CAPE_PIVOT[0],
        -0.5 * r[1][2] + CAPE_PIVOT[1],
        -0.5 * r[2][2] + CAPE_PIVOT[2],
    ]
}

/// `PlayerCapeModel.createCapeLayer`'s `PartPose` offset, mirroring
/// `rewo_gpu::mobs::CAPE_PIVOT`.
const CAPE_PIVOT: [f64; 3] = [0.0, 0.0, 2.0];

/// The cape's spine attachment point in **cape space**, for a body at
/// `y_body_rot` degrees.
///
/// The model→world chain the renderer runs is, for a direction, "negate x,
/// negate y, roll, yaw" — the negations because model +y is world down and
/// model +x is world −x. The roll is the death topple and is left out: it is
/// a render-space rotation of a corpse, and applying it to a world-space
/// chain would mean un-yawing and re-yawing every tick for an entity that is
/// about to disappear. The renderer applies it to the finished chain
/// instead.
///
/// The translation part is dropped for the reason the module header gives:
/// cape space's absolute Y is unobservable from inside the simulation.
pub fn anchor_in_cape_space(flap: f32, lean: f32, lean2: f32, y_body_rot: f32) -> [f64; 3] {
    let p = anchor_model(flap, lean, lean2);
    let e = [-p[0], -p[1], p[2]];
    let theta = (180.0 - y_body_rot as f64).to_radians();
    let (st, ct) = theta.sin_cos();
    [e[0] * ct + e[2] * st, e[1], -e[0] * st + e[2] * ct]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    fn worst_link(c: &WavyCape) -> f64 {
        (0..c.segments())
            .map(|i| (dist(c.joints()[i], c.joints()[i + 1]) - REST_LEN).abs())
            .fold(0.0, f64::max)
    }

    /// The constants are the spec's table, byte for byte.
    #[test]
    fn the_constants_are_the_specs() {
        assert_eq!(SEGMENTS, 16);
        assert_eq!(GRAVITY, 0.008);
        assert_eq!(DAMPING, 0.92);
        assert_eq!(RELAX_PASSES, 4);
        assert_eq!(REST_LEN, 1.0);
        assert_eq!(TORSO_RADIUS, 2.5);
        assert_eq!(MAX_JOINT_RADIUS, 24.0);
    }

    #[test]
    fn a_still_chain_settles_to_an_exact_fixed_point() {
        let anchor = anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
        let mut c = WavyCape::new(SEGMENTS, anchor);
        for _ in 0..600 {
            c.tick(anchor, [0.0; 3]);
        }
        let before = c.joints().to_vec();
        c.tick(anchor, [0.0; 3]);
        let drift = (0..before.len())
            .map(|j| dist(before[j], c.joints()[j]))
            .fold(0.0, f64::max);
        assert!(drift < 1e-6, "settled drift {drift:e}");
        assert!(worst_link(&c) < 1e-4, "worst link {:e}", worst_link(&c));
    }

    #[test]
    fn joint_zero_is_assigned_not_simulated() {
        let mut c = WavyCape::new(SEGMENTS, [0.0, 0.0, 2.5]);
        for t in 0..40 {
            let a = [t as f64 * 0.5, 3.0, 2.5];
            c.tick(a, [0.1, 0.0, 0.0]);
            assert_eq!(c.joints()[0], a, "tick {t}");
        }
    }

    #[test]
    fn the_clamp_catches_what_the_constraints_decline() {
        let anchor = [0.0, 0.0, 2.5];
        let mut c = WavyCape::new(SEGMENTS, anchor);
        for _ in 0..100 {
            c.tick(anchor, [0.0; 3]);
        }
        // 1e200 squared overflows to infinity, so `relax` declines every
        // link and the divergence survives to the clamp.
        c.tick(anchor, [1e200, 0.0, 0.0]);
        for (j, p) in c.joints().iter().enumerate() {
            assert!(p.iter().all(|v| v.is_finite()), "joint {j} not finite");
            assert!(
                dist(*p, anchor) <= MAX_JOINT_RADIUS + 1e-9,
                "joint {j} at {:e}",
                dist(*p, anchor)
            );
        }
        c.tick(anchor, [0.0; 3]);
        assert!(worst_link(&c) < 1e-4, "did not recover: {:e}", worst_link(&c));
    }

    #[test]
    fn interpolation_does_not_advance_the_simulation() {
        let anchor = [0.0, 0.0, 2.5];
        let mut c = WavyCape::new(SEGMENTS, anchor);
        c.tick(anchor, [0.3, 0.0, 0.1]);
        let snapshot = c.clone();
        let mut out = [[0.0f32; 3]; SEGMENTS + 1];
        for a in [0.0, 0.25, 0.5, 0.75, 1.0, 2.0, -1.0] {
            c.interpolated(a, &mut out);
        }
        assert_eq!(c, snapshot);
        c.interpolated(0.0, &mut out);
        assert_eq!(out[3][0], c.prev_joints()[3][0] as f32);
        c.interpolated(1.0, &mut out);
        assert_eq!(out[3][0], c.joints()[3][0] as f32);
    }

    #[test]
    fn the_push_out_keeps_the_chain_off_the_torso_through_a_turn() {
        let a0 = anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
        let mut c = WavyCape::new(SEGMENTS, a0);
        for _ in 0..200 {
            c.tick(a0, [0.0; 3]);
        }
        let mut worst = f64::MAX;
        for t in 0..60 {
            let yaw = (t as f32 * 30.0).min(180.0);
            let a = anchor_in_cape_space(0.0, 0.0, 0.0, yaw);
            c.tick(a, [0.0; 3]);
            for p in &c.joints()[1..] {
                worst = worst.min((p[0] * p[0] + p[2] * p[2]).sqrt());
            }
        }
        assert!(
            worst >= TORSO_RADIUS - 1e-9,
            "a joint reached radius {worst}"
        );
    }

    #[test]
    fn the_anchor_matches_the_hand_computed_rest_pose() {
        // Rx(6°)·Ry(180°) maps (0,0,−0.5) to (0, −0.5 sin6, 0.5 cos6); the
        // pivot then adds z = 2.
        let p = anchor_model(0.0, 0.0, 0.0);
        let s6 = 6f64.to_radians().sin();
        let c6 = 6f64.to_radians().cos();
        assert!((p[0]).abs() < 1e-12);
        assert!((p[1] + 0.5 * s6).abs() < 1e-12, "{p:?}");
        assert!((p[2] - (2.0 + 0.5 * c6)).abs() < 1e-12, "{p:?}");
        // Which puts it just inside the push-out cylinder, by 0.003 units.
        assert!(p[2] < TORSO_RADIUS && p[2] > TORSO_RADIUS - 0.01);
    }
}
