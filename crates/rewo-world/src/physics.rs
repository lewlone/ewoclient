//! M3 player physics — a faithful port of the vanilla 20 Hz tick, from the
//! decompiled 26.2 `LivingEntity.travel` / `Entity.move` / `LocalPlayer`
//! (REWO_PLAN.md §11 ground-truth workflow). Constants verbatim:
//!
//! - gravity 0.08/tick, vertical drag ×0.98 (`DEFAULT_BASE_GRAVITY`,
//!   `BASE_VERTICAL_AIR_DRAG`)
//! - horizontal drag ×(block_friction × 0.91); block friction 0.6 on
//!   ground, 1.0 airborne (`BASE_HORIZONTAL_AIR_DRAG`, default friction)
//! - ground accel speed = move_speed × 0.21600002 / friction³
//!   (`getFrictionInfluencedSpeed`); air accel = 0.02 (0.026 sprinting)
//! - move_speed attribute 0.1, ×1.3 sprinting; input ×0.98
//!   (`INPUT_FRICTION`), sneak ×0.3
//! - jump = 0.42 up (`JUMP_STRENGTH`), plus a 0.2 forward push when
//!   sprint-jumping (`jumpFromGround`)
//! - order per tick: accel (`moveRelative`) → collide move → gravity →
//!   drag (`travel` tail)
//!
//! M75 adds the two other movement modes the same `travel` chain reaches:
//! **flight** (`Player.travel`'s `abilities.flying` arm) and **no-clip**
//! (`Entity.move`'s `noPhysics` arm, which `Player.tick` sets from
//! `isSpectator()`). Both are branches inside this one tick, not a second
//! integrator — vanilla routes creative flight through the *ordinary*
//! `travelInAir`, and only replaces the vertical result afterwards. See
//! [`crate::abilities`] for the three conventions that read backwards.
//!
//! Movement is validated end-to-end by the bot harness: if this diverges
//! from the server's simulation, the server sends position corrections —
//! the "corrections rare" DoD is the parity meter.

use crate::abilities::Abilities;

/// Player collision box: 0.6 × 1.8 (eye height 1.62).
pub const PLAYER_HALF_WIDTH: f64 = 0.3;
pub const PLAYER_HEIGHT: f64 = 1.8;
pub const EYE_HEIGHT: f64 = 1.62;
const STEP_HEIGHT: f64 = 0.6;

const GRAVITY: f64 = 0.08;
const VERTICAL_DRAG: f64 = 0.98;
const HORIZONTAL_AIR_DRAG: f64 = 0.91;
const DEFAULT_BLOCK_FRICTION: f64 = 0.6;
const MOVE_SPEED: f64 = 0.1;
const SPRINT_MULT: f64 = 1.3;
const SNEAK_MULT: f64 = 0.3;
const INPUT_FRICTION: f64 = 0.98;
/// `Player.getFlyingSpeed()`'s non-flying arm — the airborne `moveRelative`
/// amount. Read through [`crate::abilities::Abilities::air_move_speed`], which
/// owns the flying arm as well.
pub const AIR_SPEED: f64 = 0.02;
/// The sprinting counterpart. Note it is **not** a doubling (0.02 → 0.026),
/// unlike flight's, which is.
pub const AIR_SPEED_SPRINT: f64 = 0.025999999;
const JUMP_POWER: f64 = 0.42;
/// `LivingEntity.MIN_MOVEMENT_DISTANCE` — below this a velocity component is
/// snapped to zero at the top of `aiStep`.
const MIN_MOVEMENT_DISTANCE: f64 = 0.003;

/// Per-tick input, vanilla conventions: forward +1 = W, strafe +1 = left.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TickInput {
    pub forward: f32,
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerState {
    /// Feet position (the wire position).
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl PlayerState {
    pub fn at(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            horizontal_collision: false,
        }
    }

    pub fn eye_y(&self) -> f64 {
        self.y + EYE_HEIGHT
    }
}

/// One vanilla tick, walking. `shapes(bx, by, bz)` returns that block's
/// collision boxes in block-local `0..1` — empty for no collision, one unit box
/// for a full cube, several for stairs. (Was a full-cube-only bool; partial
/// shapes arrive with the M4 model data).
///
/// Kept as the plain two-input form because it is what every existing caller
/// and parity test means: a survival player with default abilities. Flight and
/// no-clip go through [`tick_with`].
pub fn tick<'s>(
    state: &mut PlayerState,
    input: &TickInput,
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) {
    tick_with(state, input, &Abilities::default(), false, shapes);
}

/// One vanilla tick in any of the three movement modes.
///
/// `abilities` is the local player's `Abilities` — only `flying` and the flying
/// speed are read here. `no_clip` is `Entity.noPhysics`, which `Player.tick`
/// assigns from `isSpectator()`; it is deliberately *not* an ability, because
/// vanilla does not store it as one.
///
/// The three modes share one body because vanilla shares one: `Player.travel`
/// delegates to `LivingEntity.travelInAir` in **both** the flying and the
/// walking case, and differs only in what it does to the vertical result
/// afterwards.
pub fn tick_with<'s>(
    state: &mut PlayerState,
    input: &TickInput,
    abilities: &Abilities,
    no_clip: bool,
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) {
    let flying = abilities.flying;

    // -- min-movement clamp (LivingEntity.aiStep, FIRST statement) ---------
    //
    // Placement is load-bearing for flight and irrelevant for walking. Vanilla
    // clamps at the *top* of `aiStep`, which is **after** `LocalPlayer`'s
    // vertical flight impulse (added in the override, before `super.aiStep()`)
    // and before `travel`. Between two walking ticks nothing happens, so
    // clamping at the end of tick N and at the start of tick N+1 are the same
    // thing — but a flying tick has the impulse in that gap, and clamping first
    // would discard up to 0.003 of velocity the impulse should have built on.
    //
    // A **player**'s horizontal clamp is a *joint* test on the pair
    // (`horizontalDistanceSqr() < 9.0E-6`, i.e. |horizontal| < 0.003), not the
    // per-axis test every other entity gets. They differ: vx = vz = 0.0025 is
    // zeroed per-axis but survives jointly, since its magnitude is 0.00354.
    // `PlayerState` is only ever the local player, so the player branch is the
    // only correct one here.
    if state.vx * state.vx + state.vz * state.vz < 9.0e-6 {
        state.vx = 0.0;
        state.vz = 0.0;
    }
    if state.vy.abs() < MIN_MOVEMENT_DISTANCE {
        state.vy = 0.0;
    }

    // -- jump (LivingEntity.aiStep: before travel) -------------------------
    if input.jump && state.on_ground {
        state.vy = state.vy.max(JUMP_POWER);
        if input.sprint {
            let yaw = (state.yaw as f64).to_radians();
            state.vx += -yaw.sin() * 0.2;
            state.vz += yaw.cos() * 0.2;
        }
    }

    // -- acceleration (moveRelative via getFrictionInfluencedSpeed) --------
    let block_friction = if state.on_ground {
        DEFAULT_BLOCK_FRICTION
    } else {
        1.0
    };
    let speed = if state.on_ground {
        let base = MOVE_SPEED * if input.sprint { SPRINT_MULT } else { 1.0 };
        // friction == 0.6 → 0.216/0.216 = 1 → base. Kept in the vanilla
        // shape so slime/ice ports are a constant swap.
        base * (0.216_000_02 / (block_friction * block_friction * block_friction))
    } else {
        // `getFrictionInfluencedSpeed`'s airborne arm is `getFlyingSpeed()`,
        // which `Player` overrides to return the abilities' flying speed while
        // flying and the air-control constants otherwise. A flying player
        // standing on the ground takes the *walking* arm above for exactly one
        // tick, because `LocalPlayer.aiStep` ends flight on landing.
        abilities.air_move_speed(input.sprint, flying)
    };
    let mut fwd = input.forward as f64 * INPUT_FRICTION;
    let mut strafe = input.strafe as f64 * INPUT_FRICTION;
    // `LocalPlayer.modifyInput` applies `Attributes.SNEAKING_SPEED` only when
    // `isMovingSlowly()`, i.e. when crouching — and `aiStep` assigns
    // `crouching = !abilities.flying && …`. So sneaking while flying is purely
    // the descend key and costs no horizontal speed.
    if input.sneak && !flying {
        fwd *= SNEAK_MULT;
        strafe *= SNEAK_MULT;
    }
    let len_sq = fwd * fwd + strafe * strafe;
    if len_sq >= 1.0e-7 {
        let scale = if len_sq > 1.0 { 1.0 / len_sq.sqrt() } else { 1.0 } * speed;
        let (fx, fz) = (fwd * scale, strafe * scale);
        let yaw = (state.yaw as f64).to_radians();
        let (sin, cos) = (yaw.sin(), yaw.cos());
        // getInputVector: x' = strafe·cos − fwd·sin, z' = fwd·cos + strafe·sin
        state.vx += fx * -sin + fz * cos;
        state.vz += fx * cos + fz * -sin * -1.0;
    }

    // `Player.travel`'s `originalMovementY`, captured before the move. Vanilla
    // reads it one statement earlier still (before `moveRelative`), which is
    // the same value: a player's travel input has `y == 0`, so `moveRelative`
    // cannot change the vertical velocity.
    let original_vy = state.vy;

    // -- collide move (Entity.move) ---------------------------------------
    if no_clip {
        // `Entity.move`'s `noPhysics` arm: set the position and clear every
        // collision flag. `Player.tick` additionally forces `onGround = false`
        // for a spectator, which lands at the same place.
        state.x += state.vx;
        state.y += state.vy;
        state.z += state.vz;
        state.horizontal_collision = false;
        state.on_ground = false;
    } else {
        collide_move(state, shapes);
    }

    // -- gravity + drag (travel tail) --------------------------------------
    if flying {
        // `Player.travel`: `setDeltaMovement(…with(Y, originalMovementY * 0.6))`.
        //
        // `travelInAir` has already subtracted gravity and applied the 0.98
        // vertical drag by this point in vanilla — and this line discards both,
        // whole. Flight therefore has **no gravity term at all**; its only
        // vertical dynamic is this decay.
        //
        // Note `original_vy` is the pre-collision velocity: flying into a
        // ceiling does not zero it (the `move` that clipped it happened after
        // the capture), so the decay continues from what you had.
        state.vy = original_vy * crate::abilities::FLYING_VERTICAL_DECAY;
        state.vx *= block_friction * HORIZONTAL_AIR_DRAG;
        state.vz *= block_friction * HORIZONTAL_AIR_DRAG;
    } else {
        state.vy -= GRAVITY;
        state.vx *= block_friction * HORIZONTAL_AIR_DRAG;
        state.vz *= block_friction * HORIZONTAL_AIR_DRAG;
        state.vy *= VERTICAL_DRAG;
    }
    // (No trailing clamp — it is the *next* tick's first statement, above.)
}

/// Axis-separated AABB collision (vanilla order: Y, then X, then Z), with a
/// 0.6 step-up retry on horizontal block.
fn collide_move<'s>(state: &mut PlayerState, shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]]) {
    let (dx, dy, dz) = (state.vx, state.vy, state.vz);
    let (mx, my, mz) = collide(state, dx, dy, dz, shapes);

    // Step-up: horizontally blocked while on the ground → retry from +0.6.
    let blocked_x = mx != dx;
    let blocked_z = mz != dz;
    let (mx, my, mz) = if (blocked_x || blocked_z) && (state.on_ground || dy.abs() < 1e-9) {
        let (sx, sy, sz) = collide(state, dx, STEP_HEIGHT, dz, shapes);
        if sx * sx + sz * sz > mx * mx + mz * mz {
            // Settle back down onto the step.
            let mut stepped = *state;
            stepped.x += sx;
            stepped.y += sy;
            stepped.z += sz;
            let (_, down, _) = collide(&stepped, 0.0, -STEP_HEIGHT, 0.0, shapes);
            (sx, sy + down, sz)
        } else {
            (mx, my, mz)
        }
    } else {
        (mx, my, mz)
    };

    state.horizontal_collision = mx != dx || mz != dz;
    state.on_ground = my != dy && dy < 0.0;
    state.x += mx;
    state.y += my;
    state.z += mz;
    if mx != dx {
        state.vx = 0.0;
    }
    if mz != dz {
        state.vz = 0.0;
    }
    if my != dy {
        state.vy = 0.0;
    }
}

/// Clip a movement vector against solid blocks, Y then X then Z.
fn collide<'s>(
    state: &PlayerState,
    dx: f64,
    dy: f64,
    dz: f64,
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) -> (f64, f64, f64) {
    let mut min = [
        state.x - PLAYER_HALF_WIDTH,
        state.y,
        state.z - PLAYER_HALF_WIDTH,
    ];
    let mut max = [
        state.x + PLAYER_HALF_WIDTH,
        state.y + PLAYER_HEIGHT,
        state.z + PLAYER_HALF_WIDTH,
    ];
    let my = clip_axis(1, dy, &min, &max, shapes);
    min[1] += my;
    max[1] += my;
    let mx = clip_axis(0, dx, &min, &max, shapes);
    min[0] += mx;
    max[0] += mx;
    let mz = clip_axis(2, dz, &min, &max, shapes);
    (mx, my, mz)
}

/// Move an AABB along one axis, stopping at the first solid block face.
fn clip_axis<'s>(
    axis: usize,
    delta: f64,
    min: &[f64; 3],
    max: &[f64; 3],
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) -> f64 {
    if delta == 0.0 {
        return 0.0;
    }
    const EPS: f64 = 1.0e-7;
    let mut moved = delta;
    // Blocks the swept box can touch, expanded along the motion axis.
    let mut lo = [0i32; 3];
    let mut hi = [0i32; 3];
    for a in 0..3 {
        let (mut lo_f, mut hi_f) = (min[a], max[a]);
        if a == axis {
            if delta > 0.0 {
                lo_f = max[a];
                hi_f = max[a] + delta;
            } else {
                lo_f = min[a] + delta;
                hi_f = min[a];
            }
        }
        lo[a] = (lo_f - EPS).floor() as i32;
        hi[a] = (hi_f + EPS).floor() as i32;
    }
    for bx in lo[0]..=hi[0] {
        for by in lo[1]..=hi[1] {
            for bz in lo[2]..=hi[2] {
                // Each box of the block's shape clips independently — a stair
                // is two, a fence one tall post, a full cube one unit box.
                for b in shapes(bx, by, bz) {
                    let bmin = [
                        bx as f64 + b[0] as f64,
                        by as f64 + b[1] as f64,
                        bz as f64 + b[2] as f64,
                    ];
                    let bmax = [
                        bx as f64 + b[3] as f64,
                        by as f64 + b[4] as f64,
                        bz as f64 + b[5] as f64,
                    ];
                    // Overlap on the two non-motion axes?
                    let overlaps = (0..3).all(|a| {
                        a == axis || (max[a] > bmin[a] + EPS && min[a] < bmax[a] - EPS)
                    });
                    if !overlaps {
                        continue;
                    }
                    if moved > 0.0 {
                        let gap = bmin[axis] - max[axis];
                        if gap >= -EPS && gap < moved {
                            moved = gap.max(0.0);
                        }
                    } else {
                        let gap = bmax[axis] - min[axis];
                        if gap <= EPS && gap > moved {
                            moved = gap.min(0.0);
                        }
                    }
                }
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full-cube / empty shapes, so the bool worlds below read as before.
    const FULL: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    const EMPTY: &[[f32; 6]] = &[];
    fn cube(solid: bool) -> &'static [[f32; 6]] {
        if solid { FULL } else { EMPTY }
    }

    /// Flat floor: solid at y = -1 and below.
    fn floor(_x: i32, y: i32, _z: i32) -> &'static [[f32; 6]] {
        cube(y < 0)
    }

    /// Partial shapes: a floor of bottom slabs is half a block tall, so the
    /// player settles on the slab's top face — not on a full cube, and not
    /// through it. Before per-block shapes, a slab had no collision at all.
    #[test]
    fn stands_on_a_slab() {
        const SLAB: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let world = |_x: i32, y: i32, _z: i32| if y == -1 { SLAB } else { EMPTY };
        let mut p = PlayerState::at(0.5, 2.0, 0.5);
        for _ in 0..80 {
            tick(&mut p, &TickInput::default(), &world);
        }
        assert!(p.on_ground, "landed on the slab");
        assert!((p.y + 0.5).abs() < 1e-6, "settles on the slab top (y=-0.5), got {}", p.y);
    }

    /// A fence is a thin post but collides 1.5 blocks tall, so walking into
    /// one stops you — and the 0.6 step-up can't climb it.
    #[test]
    fn fence_post_blocks_movement() {
        const POST: &[[f32; 6]] = &[[0.375, 0.0, 0.375, 0.625, 1.5, 0.625]];
        let world = |_x: i32, y: i32, z: i32| {
            if y < 0 {
                FULL
            } else if z == 2 && y == 0 {
                POST
            } else {
                EMPTY
            }
        };
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        let fwd = TickInput { forward: 1.0, ..Default::default() };
        for _ in 0..60 {
            tick(&mut p, &fwd, &world);
        }
        assert!(p.horizontal_collision, "stopped by the fence post");
        assert!(p.z < 2.375 - PLAYER_HALF_WIDTH + 1e-6, "did not pass the post, z={}", p.z);
    }

    fn settle(state: &mut PlayerState) {
        for _ in 0..40 {
            tick(state, &TickInput::default(), &floor);
        }
    }

    #[test]
    fn stands_still_on_ground() {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        assert!(p.on_ground);
        assert!((p.y - 0.0).abs() < 1e-6, "rests on floor, got y={}", p.y);
        assert_eq!(p.vx, 0.0);
        assert_eq!(p.vz, 0.0);
    }

    /// Vanilla walking speed ≈ 4.317 blocks/s = 86.3 blocks over 20s.
    /// Assert the 40-tick (2 s) distance is in a tight band around 8.63.
    #[test]
    fn walk_speed_matches_vanilla() {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        let z0 = p.z;
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..40 {
            tick(&mut p, &input, &floor);
        }
        let dist = p.z - z0;
        assert!(
            (8.0..9.2).contains(&dist),
            "40-tick walk = {dist} blocks (vanilla ≈ 8.63)"
        );
    }

    /// Vanilla sprint ≈ 5.612 blocks/s → ≈ 11.2 blocks in 2 s.
    #[test]
    fn sprint_speed_matches_vanilla() {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        let z0 = p.z;
        let input = TickInput {
            forward: 1.0,
            sprint: true,
            ..Default::default()
        };
        for _ in 0..40 {
            tick(&mut p, &input, &floor);
        }
        let dist = p.z - z0;
        assert!(
            (10.4..12.0).contains(&dist),
            "40-tick sprint = {dist} blocks (vanilla ≈ 11.2)"
        );
    }

    /// Vanilla jump apex ≈ 1.2522 blocks.
    #[test]
    fn jump_apex_matches_vanilla() {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        let input = TickInput {
            jump: true,
            ..Default::default()
        };
        tick(&mut p, &input, &floor);
        let mut apex = p.y;
        for _ in 0..20 {
            tick(&mut p, &TickInput::default(), &floor);
            apex = apex.max(p.y);
        }
        assert!(
            (1.20..1.30).contains(&apex),
            "jump apex = {apex} (vanilla ≈ 1.2522)"
        );
        // And lands again.
        settle(&mut p);
        assert!(p.on_ground);
    }

    /// A **player**'s horizontal min-movement clamp is a joint test on the
    /// pair, not two independent per-axis tests. `vx = vz = 0.0025` is below
    /// 0.003 on each axis but has magnitude 0.00354, so vanilla keeps it and a
    /// per-axis clamp would zero both.
    ///
    /// `EntityTypes.PLAYER` takes the `horizontalDistanceSqr() < 9.0E-6` arm;
    /// every other entity takes the per-axis one. `PlayerState` is only ever
    /// the local player.
    #[test]
    fn the_horizontal_clamp_is_joint_for_a_player() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let survives = |vx: f64, vz: f64| {
            let mut p = PlayerState::at(0.5, 80.0, 0.5);
            p.vx = vx;
            p.vz = vz;
            // One tick: the clamp is its first statement. Airborne drag is
            // 0.91, so a surviving component stays clearly non-zero.
            tick(&mut p, &TickInput::default(), &air);
            (p.vx != 0.0, p.vz != 0.0)
        };
        // Magnitude 0.00354 > 0.003 — both survive jointly.
        assert_eq!(survives(0.0025, 0.0025), (true, true), "a per-axis clamp would zero both");
        // Magnitude 0.00283 < 0.003 — both go.
        assert_eq!(survives(0.002, 0.002), (false, false));
        // One axis alone, above the threshold: survives.
        assert_eq!(survives(0.004, 0.0), (true, false));
    }

    /// The clamp runs at the *top* of the tick, so the velocity a caller reads
    /// straight after `tick` is deliberately unclamped — it is the next tick's
    /// job. This pins the placement rather than the values.
    #[test]
    fn the_clamp_is_the_first_statement_not_the_last() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        p.vy = 0.001; // below the threshold
        p.vx = 0.001;
        p.vz = 0.0;
        tick(&mut p, &TickInput::default(), &air);
        // Cleared on entry, so vy is now pure gravity+drag and vx stayed 0.
        assert_eq!(p.vx, 0.0);
        assert!((p.vy + 0.08 * 0.98).abs() < 1e-12, "vy={} — entered at 0", p.vy);
    }

    // --------------------------------------------------------- M75: flight

    fn flying() -> Abilities {
        let mut a = Abilities::default();
        a.mayfly = true;
        a.flying = true;
        a
    }

    /// Flight's whole vertical dynamic is `vy ← vy_before_move × 0.6`. With no
    /// input that is a pure geometric decay — and, decisively, it does **not**
    /// accelerate downward, because the gravity `travelInAir` computes is
    /// discarded.
    #[test]
    fn flight_has_no_gravity_and_decays_vertically_by_0_6() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        p.vy = 1.0;
        let a = flying();
        let mut seen = Vec::new();
        for _ in 0..5 {
            tick_with(&mut p, &TickInput::default(), &a, false, &air);
            seen.push(p.vy);
        }
        // 1.0 → 0.6 → 0.36 → 0.216 → 0.1296 → 0.07776, each exactly ×0.6.
        let want = [0.6, 0.36, 0.216, 0.1296, 0.07776];
        for (got, w) in seen.iter().zip(want) {
            assert!((got - w).abs() < 1e-12, "got {seen:?}, want {want:?}");
        }
        // A walking player would be falling by now; this one is still rising.
        assert!(p.vy > 0.0, "no gravity term while flying");
    }

    /// A *walking* player in the same air is the control: gravity dominates
    /// within a few ticks. This is the sensitivity partner for the test above —
    /// if the flight branch were not taken, that test's numbers could not hold.
    #[test]
    fn the_same_state_walking_falls_instead() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        p.vy = 1.0;
        for _ in 0..5 {
            tick(&mut p, &TickInput::default(), &air);
        }
        assert!(p.vy < 0.6, "walking, gravity has bitten: vy={}", p.vy);
    }

    /// Holding jump reaches a fixed point where `v = (v + I)·0.6`, i.e.
    /// `v = 1.5·I`. With the default 0.05 flying speed the impulse is 0.15, so
    /// the carried velocity settles at 0.225 and the per-tick ascent — which is
    /// the velocity the move actually uses, *including* this tick's impulse —
    /// settles at 0.375 blocks/tick.
    #[test]
    fn flight_ascent_reaches_its_closed_form_terminal() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        let a = flying();
        let up = TickInput {
            jump: true,
            ..Default::default()
        };
        let mut fc = crate::abilities::FlightControl::default();
        let mut ab = a;
        for _ in 0..200 {
            fc.before_travel(&mut ab, &mut p, &up, false, false);
            tick_with(&mut p, &up, &ab, false, &air);
        }
        // I is the f32-computed 0.15f, so the fixed point is 1.5·I exactly.
        let i = crate::abilities::Abilities::default().vertical_flight_impulse(true, false);
        assert!((p.vy - 1.5 * i).abs() < 1e-12, "carried vy={}, want 1.5·I", p.vy);
        let before = p.y;
        fc.before_travel(&mut ab, &mut p, &up, false, false);
        tick_with(&mut p, &up, &ab, false, &air);
        assert!(
            (p.y - before - 2.5 * i).abs() < 1e-12,
            "per-tick ascent = {}, want 1.5·I + I",
            p.y - before
        );
        // ≈ 0.375 blocks/tick = 7.5 blocks/s.
        assert!((p.y - before - 0.375).abs() < 1e-7);
    }

    /// Horizontal flight uses the *flying* accel (0.05, doubled sprinting)
    /// against the ordinary 0.91 air drag.
    ///
    /// The fixed point is `v = (v + a)·0.91`, so `v = 0.91a/(1 − 0.91)` —
    /// **not** `a/(1 − 0.91)`: the drag applies to the accelerated velocity, not
    /// to the carried one. Writing it the intuitive way overstates the terminal
    /// by 1/0.91 ≈ 10%, which is the size of error a "looks about right" eyeball
    /// would pass.
    #[test]
    fn flight_horizontal_terminal_matches_its_closed_form() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let terminal = |sprint: bool| {
            let mut p = PlayerState::at(0.5, 80.0, 0.5);
            let a = flying();
            let input = TickInput {
                forward: 1.0,
                sprint,
                ..Default::default()
            };
            for _ in 0..400 {
                tick_with(&mut p, &input, &a, false, &air);
            }
            p.vz
        };
        let want = |accel: f64| {
            accel * INPUT_FRICTION * HORIZONTAL_AIR_DRAG / (1.0 - HORIZONTAL_AIR_DRAG)
        };
        let base = f64::from(0.05f32);
        assert!((terminal(false) - want(base)).abs() < 1e-6, "{}", terminal(false));
        // Sprinting is a clean doubling here — unlike the walking air constants.
        assert!((terminal(true) - want(base * 2.0)).abs() < 1e-6, "{}", terminal(true));
        // ≈ 0.4954 blocks/tick = 9.9 blocks/s, and ≈ 19.8 sprinting.
        assert!((terminal(false) - 0.4954).abs() < 1e-3, "{}", terminal(false));
    }

    /// Sneak is the descend key while flying, and must not also apply the 0.3
    /// crouch factor — `crouching` is `!flying && …`.
    #[test]
    fn sneaking_does_not_slow_a_flying_player() {
        let air = |_x: i32, _y: i32, _z: i32| EMPTY;
        let run = |sneak: bool| {
            let mut p = PlayerState::at(0.5, 80.0, 0.5);
            let a = flying();
            let input = TickInput {
                forward: 1.0,
                sneak,
                ..Default::default()
            };
            for _ in 0..200 {
                tick_with(&mut p, &input, &a, false, &air);
            }
            p.vz
        };
        assert!((run(true) - run(false)).abs() < 1e-12, "{} vs {}", run(true), run(false));

        // Control: walking, the same key *does* slow you — so the test above is
        // measuring the flight branch and not an inert flag.
        let ground = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let walk = |sneak: bool| {
            let mut p = PlayerState::at(0.5, 0.0, 0.5);
            settle(&mut p);
            let input = TickInput {
                forward: 1.0,
                sneak,
                ..Default::default()
            };
            for _ in 0..40 {
                tick(&mut p, &input, &ground);
            }
            p.vz
        };
        assert!(walk(true) < walk(false) * 0.5, "{} vs {}", walk(true), walk(false));
    }

    /// A flying player still collides — flight is not no-clip. (Spectator is,
    /// and that is a separate flag.)
    #[test]
    fn flight_still_collides_but_no_clip_does_not() {
        // Floor at y < 0, ceiling at y >= 3.
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0 || y >= 3);
        let mut p = PlayerState::at(0.5, 1.0, 0.5);
        let a = flying();
        let down = TickInput {
            sneak: true,
            ..Default::default()
        };
        let mut fc = crate::abilities::FlightControl::default();
        let mut ab = a;
        for _ in 0..60 {
            fc.before_travel(&mut ab, &mut p, &down, false, false);
            tick_with(&mut p, &down, &ab, false, &world);
        }
        assert!(p.y >= -1e-9, "flying down stops at the floor, y={}", p.y);

        // The same descent with no-clip sinks straight through it.
        let mut q = PlayerState::at(0.5, 1.0, 0.5);
        let mut fc2 = crate::abilities::FlightControl::default();
        let mut ab2 = flying();
        for _ in 0..60 {
            fc2.before_travel(&mut ab2, &mut q, &down, false, false);
            tick_with(&mut q, &down, &ab2, true, &world);
        }
        assert!(q.y < -5.0, "no-clip passes through the floor, y={}", q.y);
        assert!(!q.on_ground && !q.horizontal_collision, "flags cleared");
    }

    /// The default-abilities [`tick`] wrapper must be the walking path exactly —
    /// this is what keeps every pre-M75 caller and parity number unchanged.
    #[test]
    fn tick_is_tick_with_default_abilities() {
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let input = TickInput {
            forward: 1.0,
            jump: true,
            sprint: true,
            ..Default::default()
        };
        let mut a = PlayerState::at(0.5, 0.0, 0.5);
        let mut b = PlayerState::at(0.5, 0.0, 0.5);
        for _ in 0..120 {
            tick(&mut a, &input, &world);
            tick_with(&mut b, &input, &Abilities::default(), false, &world);
        }
        assert_eq!((a.x, a.y, a.z, a.vx, a.vy, a.vz), (b.x, b.y, b.z, b.vx, b.vy, b.vz));
    }

    #[test]
    fn wall_blocks_and_sets_collision_flag() {
        // Floor plus a wall at z = 2.
        let world = |x: i32, y: i32, z: i32| cube(y < 0 || (z == 2 && y < 3 && x.abs() < 8));
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        for _ in 0..40 {
            tick(&mut p, &TickInput::default(), &world);
        }
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..40 {
            tick(&mut p, &input, &world);
        }
        assert!(p.horizontal_collision);
        assert!(
            p.z < 2.0 - PLAYER_HALF_WIDTH + 1e-6,
            "stopped at the wall, z={}",
            p.z
        );
    }

    /// Vanilla step height is 0.6 — a full block can NOT be walked up; it
    /// takes a jump. Hold forward + jump and land on the ledge.
    #[test]
    fn jumps_up_single_block_ledge() {
        // Floor, with a raised floor (one block higher) from z >= 3.
        let world = |_x: i32, y: i32, z: i32| cube(if z >= 3 { y < 1 } else { y < 0 });
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        for _ in 0..40 {
            tick(&mut p, &TickInput::default(), &world);
        }
        let input = TickInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        for _ in 0..80 {
            tick(&mut p, &input, &world);
        }
        assert!(p.z > 4.0, "made it onto the ledge, z={}", p.z);
        assert!((p.y - 1.0).abs() < 0.35, "standing on the ledge, y={}", p.y);

        // And confirm walking alone does NOT climb it (parity guard).
        let mut q = PlayerState::at(0.5, 0.0, 0.5);
        for _ in 0..40 {
            tick(&mut q, &TickInput::default(), &world);
        }
        let walk = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..80 {
            tick(&mut q, &walk, &world);
        }
        assert!(q.y < 0.5, "walking must not scale a full block, y={}", q.y);
    }
}
