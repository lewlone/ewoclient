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
//! Movement is validated end-to-end by the bot harness: if this diverges
//! from the server's simulation, the server sends position corrections —
//! the "corrections rare" DoD is the parity meter.

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
const AIR_SPEED: f64 = 0.02;
const AIR_SPEED_SPRINT: f64 = 0.025999999;
const JUMP_POWER: f64 = 0.42;

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

/// One vanilla tick. `solid(bx, by, bz)` answers "is this block a full
/// collision cube" — M3 collides against full cubes only (partial shapes
/// arrive with the M4 model data).
pub fn tick(state: &mut PlayerState, input: &TickInput, solid: &dyn Fn(i32, i32, i32) -> bool) {
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
    } else if input.sprint {
        AIR_SPEED_SPRINT
    } else {
        AIR_SPEED
    };
    let mut fwd = input.forward as f64 * INPUT_FRICTION;
    let mut strafe = input.strafe as f64 * INPUT_FRICTION;
    if input.sneak {
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

    // -- collide move (Entity.move) ---------------------------------------
    collide_move(state, solid);

    // -- gravity + drag (travel tail) --------------------------------------
    state.vy -= GRAVITY;
    state.vx *= block_friction * HORIZONTAL_AIR_DRAG;
    state.vz *= block_friction * HORIZONTAL_AIR_DRAG;
    state.vy *= VERTICAL_DRAG;
    if state.vx.abs() < 0.003 {
        state.vx = 0.0;
    }
    if state.vz.abs() < 0.003 {
        state.vz = 0.0;
    }
    if state.vy.abs() < 0.003 {
        state.vy = 0.0;
    }
}

/// Axis-separated AABB collision (vanilla order: Y, then X, then Z), with a
/// 0.6 step-up retry on horizontal block.
fn collide_move(state: &mut PlayerState, solid: &dyn Fn(i32, i32, i32) -> bool) {
    let (dx, dy, dz) = (state.vx, state.vy, state.vz);
    let (mx, my, mz) = collide(state, dx, dy, dz, solid);

    // Step-up: horizontally blocked while on the ground → retry from +0.6.
    let blocked_x = mx != dx;
    let blocked_z = mz != dz;
    let (mx, my, mz) = if (blocked_x || blocked_z) && (state.on_ground || dy.abs() < 1e-9) {
        let (sx, sy, sz) = collide(state, dx, STEP_HEIGHT, dz, solid);
        if sx * sx + sz * sz > mx * mx + mz * mz {
            // Settle back down onto the step.
            let mut stepped = *state;
            stepped.x += sx;
            stepped.y += sy;
            stepped.z += sz;
            let (_, down, _) = collide(&stepped, 0.0, -STEP_HEIGHT, 0.0, solid);
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
fn collide(
    state: &PlayerState,
    dx: f64,
    dy: f64,
    dz: f64,
    solid: &dyn Fn(i32, i32, i32) -> bool,
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
    let my = clip_axis(1, dy, &min, &max, solid);
    min[1] += my;
    max[1] += my;
    let mx = clip_axis(0, dx, &min, &max, solid);
    min[0] += mx;
    max[0] += mx;
    let mz = clip_axis(2, dz, &min, &max, solid);
    (mx, my, mz)
}

/// Move an AABB along one axis, stopping at the first solid block face.
fn clip_axis(
    axis: usize,
    delta: f64,
    min: &[f64; 3],
    max: &[f64; 3],
    solid: &dyn Fn(i32, i32, i32) -> bool,
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
                if !solid(bx, by, bz) {
                    continue;
                }
                let bmin = [bx as f64, by as f64, bz as f64];
                let bmax = [bmin[0] + 1.0, bmin[1] + 1.0, bmin[2] + 1.0];
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
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat floor: solid at y = -1 and below.
    fn floor(_x: i32, y: i32, _z: i32) -> bool {
        y < 0
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

    #[test]
    fn wall_blocks_and_sets_collision_flag() {
        // Floor plus a wall at z = 2.
        let world = |x: i32, y: i32, z: i32| -> bool { y < 0 || (z == 2 && y < 3 && x.abs() < 8) };
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
        let world = |_x: i32, y: i32, z: i32| -> bool {
            if z >= 3 {
                y < 1
            } else {
                y < 0
            }
        };
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
