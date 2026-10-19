//! A boat the local player steers (M221): `AbstractBoat.tick` on the client,
//! where vanilla runs it for an entity the local player controls
//! (`isLocalInstanceAuthoritative`) and sends the result as
//! `ServerboundMoveVehiclePacket`.
//!
//! Transcribed, in `tick`'s order: the water status (`getStatus` and its
//! three scans), `Entity.baseTick`'s water current, `floatBoat`
//! (gravity, buoyancy, friction by status), `controlBoat` (the four keys'
//! rotation and thrust, and the paddle states), and `Entity.move` for a boat
//! (collision with no step height, the collision flags, `checkFallDamage`'s
//! `lastYd`, the bounce rule for a non-living entity, the block speed
//! factor).
//!
//! Not modelled, each recorded: bubble columns (`applyEffectsFromBlocks`,
//! `tickBubbleColumn`), pushing other entities, the out-of-control eject
//! (server-side), and `getGroundFriction`'s lily-pad exception (a boat over
//! lily pads is in water, where ground friction is not read).

use crate::lightmap::{mth_cos, mth_sin};
use crate::physics::{
    collect_colliders, collide_with_shapes, expand_towards, fluid_top, inflate, intersects, len_sq,
    mth_equal, offset, Aabb, PhysicsWorld, EPS,
};
use rewo_data::block_physics::PhysFluid;

/// `EntityType.BOAT` and every other boat and raft: `sized(1.375F, 0.5625F)`.
pub const BOAT_WIDTH: f64 = 1.375;
pub const BOAT_HEIGHT: f64 = 0.5625;
/// `AbstractBoat.getDefaultGravity`.
const GRAVITY: f64 = 0.04;

/// `AbstractBoat.Status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    #[default]
    InAir,
}

/// The boat's simulated state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoatState {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    /// `getYRot()`, degrees.
    pub yaw: f32,
    /// `deltaRotation` — what `controlBoat` turns by each tick, also added to
    /// the rider's yaw (`positionRider`).
    pub delta_rotation: f32,
    pub status: Status,
    pub old_status: Status,
    pub water_level: f64,
    pub land_friction: f32,
    /// `lastYd`, the vertical speed `checkFallDamage` saw.
    pub last_yd: f64,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    /// `DATA_ID_PADDLE_LEFT` / `RIGHT`, what `ServerboundPaddleBoatPacket`
    /// carries.
    pub paddles: [bool; 2],
}

impl BoatState {
    /// A boat at rest where the server put it.
    pub fn at(x: f64, y: f64, z: f64, yaw: f32) -> Self {
        Self {
            x,
            y,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw,
            delta_rotation: 0.0,
            status: Status::InAir,
            old_status: Status::InAir,
            water_level: 0.0,
            land_friction: 0.0,
            last_yd: 0.0,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            paddles: [false; 2],
        }
    }

    pub fn aabb(&self) -> Aabb {
        let h = BOAT_WIDTH / 2.0;
        [
            self.x - h,
            self.y,
            self.z - h,
            self.x + h,
            self.y + BOAT_HEIGHT,
            self.z + h,
        ]
    }
}

/// `LocalPlayer.rideTick`'s `boat.setInput(left, right, forward, backward)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoatInput {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
}

/// One `AbstractBoat.tick` for a boat the local player controls.
pub fn tick(s: &mut BoatState, input: BoatInput, world: &dyn PhysicsWorld) {
    s.old_status = s.status;
    s.status = status(s, world);
    // `super.tick()` → `Entity.baseTick` → the water current.
    push_by_water(s, world);
    float_boat(s, world);
    control_boat(s, input);
    let (vx, vy, vz) = (s.vx, s.vy, s.vz);
    move_boat(s, world, [vx, vy, vz]);
}

/// `AbstractBoat.getStatus`.
fn status(s: &mut BoatState, world: &dyn PhysicsWorld) -> Status {
    if let Some(st) = underwater(s, world) {
        s.water_level = s.aabb()[4];
        return st;
    }
    if check_in_water(s, world) {
        return Status::InWater;
    }
    let friction = ground_friction(s, world);
    // `friction / count` is 0/0 = NaN with nothing under the boat, which is
    // not `> 0` — IN_AIR, as in Java.
    if friction > 0.0 {
        s.land_friction = friction;
        Status::OnLand
    } else {
        Status::InAir
    }
}

fn is_water(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32) -> bool {
    matches!(world.block(x, y, z).fluid, PhysFluid::Water { .. })
}

/// `FluidState.getHeight(level, pos)` for water, relative to the block.
fn water_height(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32) -> Option<f64> {
    match fluid_top(world, x, y, z) {
        Some((false, top)) => Some(top - y as f64),
        _ => None,
    }
}

/// `AbstractBoat.isUnderwater`.
fn underwater(s: &BoatState, world: &dyn PhysicsWorld) -> Option<Status> {
    let b = s.aabb();
    let max_y = b[4] + 0.001;
    let (x0, x1) = (b[0].floor() as i32, b[3].ceil() as i32);
    let (y0, y1) = (b[4].floor() as i32, max_y.ceil() as i32);
    let (z0, z1) = (b[2].floor() as i32, b[5].ceil() as i32);
    let mut under = false;
    for x in x0..x1 {
        for y in y0..y1 {
            for z in z0..z1 {
                if let Some(h) = water_height(world, x, y, z) {
                    if max_y < y as f64 + h {
                        // `isSource()`: a full, non-falling cell.
                        let block = world.block(x, y, z);
                        let source = matches!(block.fluid, PhysFluid::Water { own_height } if own_height >= 0.8888888)
                            && !block.fluid_falling;
                        if !source {
                            return Some(Status::UnderFlowingWater);
                        }
                        under = true;
                    }
                }
            }
        }
    }
    under.then_some(Status::UnderWater)
}

/// `AbstractBoat.checkInWater`, which also sets `waterLevel`.
fn check_in_water(s: &mut BoatState, world: &dyn PhysicsWorld) -> bool {
    let b = s.aabb();
    let (x0, x1) = (b[0].floor() as i32, b[3].ceil() as i32);
    let (y0, y1) = (b[1].floor() as i32, (b[1] + 0.001).ceil() as i32);
    let (z0, z1) = (b[2].floor() as i32, b[5].ceil() as i32);
    let mut in_water = false;
    s.water_level = -f64::MAX;
    for x in x0..x1 {
        for y in y0..y1 {
            for z in z0..z1 {
                if let Some(h) = water_height(world, x, y, z) {
                    // `float height = y + fluidState.getHeight(..)`.
                    let height = (y as f64 + h) as f32 as f64;
                    s.water_level = s.water_level.max(height);
                    in_water |= b[1] < height;
                }
            }
        }
    }
    in_water
}

/// `AbstractBoat.getWaterLevelAbove`.
fn water_level_above(s: &BoatState, world: &dyn PhysicsWorld) -> f64 {
    let b = s.aabb();
    let (x0, x1) = (b[0].floor() as i32, b[3].ceil() as i32);
    let (y0, y1) = (b[4].floor() as i32, (b[4] - s.last_yd).ceil() as i32);
    let (z0, z1) = (b[2].floor() as i32, b[5].ceil() as i32);
    let mut y = y0;
    'rows: while y < y1 {
        let mut block_height = 0.0f32;
        for x in x0..x1 {
            for z in z0..z1 {
                if is_water(world, x, y, z) {
                    let h = water_height(world, x, y, z).unwrap_or(0.0) as f32;
                    block_height = block_height.max(h);
                }
                if block_height >= 1.0 {
                    y += 1;
                    continue 'rows;
                }
            }
        }
        if block_height < 1.0 {
            return y as f64 + block_height as f64;
        }
        y += 1;
    }
    (y1 + 1) as f64
}

/// `AbstractBoat.getGroundFriction`: the mean friction of the blocks whose
/// collision touches a 0.001-thick slab under the boat, skipping the scan's
/// corner columns and, on its edges, the top and bottom rows.
fn ground_friction(s: &BoatState, world: &dyn PhysicsWorld) -> f32 {
    let bb = s.aabb();
    let slab: Aabb = [bb[0], bb[1] - 0.001, bb[2], bb[3], bb[1], bb[5]];
    let x0 = slab[0].floor() as i32 - 1;
    let x1 = slab[3].ceil() as i32 + 1;
    let y0 = slab[1].floor() as i32 - 1;
    let y1 = slab[4].ceil() as i32 + 1;
    let z0 = slab[2].floor() as i32 - 1;
    let z1 = slab[5].ceil() as i32 + 1;
    let mut friction = 0.0f32;
    let mut count = 0i32;
    for x in x0..x1 {
        for z in z0..z1 {
            let edges = (x == x0 || x == x1 - 1) as i32 + (z == z0 || z == z1 - 1) as i32;
            if edges == 2 {
                continue;
            }
            for y in y0..y1 {
                if edges > 0 && (y == y0 || y == y1 - 1) {
                    continue;
                }
                let touches = world.collision(x, y, z).iter().any(|c| {
                    let b: Aabb = [
                        x as f64 + c[0] as f64,
                        y as f64 + c[1] as f64,
                        z as f64 + c[2] as f64,
                        x as f64 + c[3] as f64,
                        y as f64 + c[4] as f64,
                        z as f64 + c[5] as f64,
                    ];
                    intersects(&b, &slab)
                });
                if touches {
                    friction += world.block(x, y, z).friction;
                    count += 1;
                }
            }
        }
    }
    friction / count as f32
}

/// `Entity.updateFluidHeightAndDoFluidPushing` for water at 0.014 a tick, for
/// a non-player: the mean flow of the cells the box (deflated by 0.001)
/// reaches into, NORMALIZED, scaled, with the weak-current nudge.
fn push_by_water(s: &mut BoatState, world: &dyn PhysicsWorld) {
    let b = inflate(&s.aabb(), -0.001);
    let (x0, x1) = (b[0].floor() as i32, b[3].ceil() as i32);
    let (y0, y1) = (b[1].floor() as i32, b[4].ceil() as i32);
    let (z0, z1) = (b[2].floor() as i32, b[5].ceil() as i32);
    let mut sum = [0.0f64; 3];
    let mut count = 0;
    for x in x0..x1 {
        for y in y0..y1 {
            for z in z0..z1 {
                let Some(h) = water_height(world, x, y, z) else {
                    continue;
                };
                let top = y as f64 + h;
                if top < b[1] {
                    continue;
                }
                let mut flow = crate::physics::fluid_flow(world, x, y, z, false);
                let depth = top - b[1];
                if depth < 0.4 {
                    flow = [flow[0] * depth, flow[1] * depth, flow[2] * depth];
                }
                sum = [sum[0] + flow[0], sum[1] + flow[1], sum[2] + flow[2]];
                count += 1;
            }
        }
    }
    if count == 0 || len_sq(sum) < 1.0e-5f32 as f64 {
        return;
    }
    let mean = [
        sum[0] / count as f64,
        sum[1] / count as f64,
        sum[2] / count as f64,
    ];
    // A non-player's current is the normalized direction.
    let l = len_sq(mean).sqrt();
    let dir = if l < 1.0e-5 {
        [0.0; 3]
    } else {
        [mean[0] / l, mean[1] / l, mean[2] / l]
    };
    let mut impulse = [dir[0] * 0.014, dir[1] * 0.014, dir[2] * 0.014];
    if s.vx.abs() < 0.003 && s.vz.abs() < 0.003 && len_sq(impulse).sqrt() < 0.0045000000000000005 {
        let il = len_sq(impulse).sqrt();
        if il > 1.0e-5 {
            let k = 0.0045000000000000005 / il;
            impulse = [impulse[0] * k, impulse[1] * k, impulse[2] * k];
        }
    }
    s.vx += impulse[0];
    s.vy += impulse[1];
    s.vz += impulse[2];
}

/// `Level.noCollision(entity, box)` for blocks and the entities a boat stands
/// on.
fn no_collision(world: &dyn PhysicsWorld, b: &Aabb) -> bool {
    collect_colliders(b, world)
        .iter()
        .all(|shape| shape.iter().all(|c| !intersects(c, b)))
        && world.entity_colliders(inflate(b, EPS)).is_empty()
}

/// `AbstractBoat.floatBoat`.
fn float_boat(s: &mut BoatState, world: &dyn PhysicsWorld) {
    if s.old_status == Status::InAir && s.status != Status::InAir && s.status != Status::OnLand {
        // `waterLevel = getY(1.0)`.
        s.water_level = s.y + BOAT_HEIGHT;
        let target_y = water_level_above(s, world) - BOAT_HEIGHT + 0.101;
        let moved = offset(&s.aabb(), [0.0, target_y - s.y, 0.0]);
        if no_collision(world, &moved) {
            s.y = target_y;
            s.vy = 0.0;
            s.last_yd = 0.0;
        }
        s.status = Status::InWater;
        return;
    }
    // `(vspeed, buoyancy, invFriction)`; vanilla's initial `invFriction =
    // 0.05F` is never read, every status assigns its own.
    let (vspeed, buoyancy, inv_friction) = match s.status {
        Status::InWater => (-GRAVITY, (s.water_level - s.y) / BOAT_HEIGHT, 0.9f32),
        Status::UnderFlowingWater => (-7.0e-4, 0.0, 0.9),
        Status::UnderWater => (-GRAVITY, 0.01f32 as f64, 0.45),
        Status::InAir => (-GRAVITY, 0.0, 0.9),
        Status::OnLand => {
            let f = s.land_friction;
            // The controlling passenger is the local player. (`getStatus`
            // re-reads the ground every tick, so this halving is overwritten
            // before it is read again.)
            s.land_friction /= 2.0;
            (-GRAVITY, 0.0, f)
        }
    };
    s.vx *= inv_friction as f64;
    s.vy += vspeed;
    s.vz *= inv_friction as f64;
    s.delta_rotation *= inv_friction;
    if buoyancy > 0.0 {
        s.vy = (s.vy + buoyancy * (GRAVITY / 0.65)) * 0.75;
    }
}

/// `AbstractBoat.controlBoat` (the boat has a passenger: the local player).
fn control_boat(s: &mut BoatState, input: BoatInput) {
    let mut acceleration = 0.0f32;
    if input.left {
        s.delta_rotation -= 1.0;
    }
    if input.right {
        s.delta_rotation += 1.0;
    }
    if input.right != input.left && !input.up && !input.down {
        acceleration += 0.005;
    }
    s.yaw += s.delta_rotation;
    if input.up {
        acceleration += 0.04;
    }
    if input.down {
        acceleration -= 0.005;
    }
    let rad = (std::f64::consts::PI / 180.0) as f32;
    s.vx += (mth_sin(-s.yaw * rad) * acceleration) as f64;
    s.vz += (mth_cos(s.yaw * rad) * acceleration) as f64;
    s.paddles = [
        input.right && !input.left || input.up,
        input.left && !input.right || input.up,
    ];
}

/// `Entity.move(MoverType.SELF, delta)` for a boat: no step height, no edge
/// back-off, bounciness 0 of its own (a slime block still bounces it at 0.8
/// of its restitution — `getBlockBounciness` for a non-living entity).
fn move_boat(s: &mut BoatState, world: &dyn PhysicsWorld, delta: [f64; 3]) {
    let bx = s.aabb();
    let q = expand_towards(&bx, delta);
    let mut colliders: Vec<Vec<Aabb>> = world
        .entity_colliders(inflate(&q, EPS))
        .into_iter()
        .map(|b| vec![b])
        .collect();
    colliders.extend(collect_colliders(&q, world));
    let movement = if len_sq(delta) == 0.0 {
        delta
    } else {
        collide_with_shapes(delta, &bx, &colliders, None)
    };
    let moved_sq = len_sq(movement);
    if moved_sq > 1.0e-7 || len_sq(delta) - moved_sq < 1.0e-7 {
        s.x += movement[0];
        s.y += movement[1];
        s.z += movement[2];
    }
    let x_coll = !mth_equal(delta[0], movement[0]);
    let z_coll = !mth_equal(delta[2], movement[2]);
    s.horizontal_collision = x_coll || z_coll;
    s.vertical_collision = delta[1] != movement[1];
    let below = s.vertical_collision && delta[1] < 0.0;
    s.on_ground = below;
    // `checkFallDamage` (local instance authoritative).
    s.last_yd = s.vy;
    let moved_vertically = delta[1].abs() > 0.0;
    if moved_vertically && s.vertical_collision || s.horizontal_collision {
        // `restituteMovementAfterCollisions` with the entity's own 0.
        if x_coll {
            s.vx = -s.vx * 0.0;
        }
        if z_coll {
            s.vz = -s.vz * 0.0;
        }
        if s.vertical_collision {
            let on = on_pos(s, world);
            let block_bounce = world.block(on[0], on[1], on[2]).bounce as f64 * 0.8f32 as f64;
            let restitution = if below && !(-s.vy < GRAVITY) {
                block_bounce
            } else {
                0.0
            };
            // `effectiveDrag` lerps toward `getAirDrag()`, which is 1 for a
            // boat, so it is 1 either way.
            let comp = if restitution > 0.0 {
                movement[1] / s.vy * GRAVITY
            } else {
                0.0
            };
            s.vy = (comp - s.vy) * restitution;
        }
    }
    let f = block_speed_factor(s, world) as f64;
    s.vx *= f;
    s.vz *= f;
}

/// `Entity.getOnPosLegacy`: the block 0.2 under the feet.
fn on_pos(s: &BoatState, _world: &dyn PhysicsWorld) -> [i32; 3] {
    [
        s.x.floor() as i32,
        (s.y - 0.2).floor() as i32,
        s.z.floor() as i32,
    ]
}

/// `Entity.getBlockSpeedFactor`: the feet block's factor, or — when that is
/// 1 and it is not water — the factor of the block under the feet.
fn block_speed_factor(s: &BoatState, world: &dyn PhysicsWorld) -> f32 {
    let (x, y, z) = (s.x.floor() as i32, s.y.floor() as i32, s.z.floor() as i32);
    let here = world.block(x, y, z);
    if matches!(here.fluid, PhysFluid::Water { .. }) || here.speed_factor != 1.0 {
        return here.speed_factor;
    }
    world
        .block(x, (s.y - 0.500001).floor() as i32, z)
        .speed_factor
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_data::block_physics::BlockPhysics;

    /// A flat world: stone up to y = -1, then (optionally) still water at
    /// y = 0, air above.
    struct Pond {
        water: bool,
        ice: bool,
    }

    const FULL: [[f32; 6]; 1] = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

    impl PhysicsWorld for Pond {
        fn collision(&self, _x: i32, y: i32, _z: i32) -> &[[f32; 6]] {
            if y < 0 {
                &FULL
            } else {
                &[]
            }
        }
        fn block(&self, _x: i32, y: i32, _z: i32) -> BlockPhysics {
            let mut b = BlockPhysics::default();
            if y == -1 && self.ice {
                b.friction = 0.98;
            }
            if y == 0 && self.water {
                b.fluid = PhysFluid::Water {
                    own_height: 8.0 / 9.0,
                };
            }
            b
        }
        fn has_chunk(&self, _x: i32, _z: i32) -> bool {
            true
        }
        fn min_y(&self) -> i32 {
            -64
        }
    }

    /// A boat dropped onto still water settles where `floatBoat`'s two terms
    /// cancel: `-0.04 + b * 0.04 / 0.65 = 0` with `b = (waterLevel - y) /
    /// height`, i.e. `y = surface - 0.65 * 0.5625`. (The landing snap's
    /// `surface - height + 0.101` is only where it starts.)
    #[test]
    fn a_boat_floats_on_still_water() {
        let world = Pond {
            water: true,
            ice: false,
        };
        let mut s = BoatState::at(0.5, 0.5, 0.5, 0.0);
        for _ in 0..200 {
            tick(&mut s, BoatInput::default(), &world);
        }
        assert_eq!(s.status, Status::InWater);
        // `checkInWater`'s `float height`.
        let surface = (8.0f32 / 9.0) as f64;
        let rest = surface - 0.65 * BOAT_HEIGHT;
        assert!(
            (s.y - rest).abs() < 1e-3,
            "settled at {} (rest {rest})",
            s.y
        );
        assert!(s.vx.abs() < 1e-9 && s.vz.abs() < 1e-9);
    }

    /// The landing snap: the first tick a falling boat meets water it is put
    /// at `getWaterLevelAbove() - height + 0.101` with no vertical speed.
    #[test]
    fn landing_on_water_snaps_to_the_float_height() {
        let world = Pond {
            water: true,
            ice: false,
        };
        let mut s = BoatState::at(0.5, 0.95, 0.5, 0.0);
        tick(&mut s, BoatInput::default(), &world); // in air: falls into the water
        let mut landed = None;
        for _ in 0..20 {
            let before = s.status;
            tick(&mut s, BoatInput::default(), &world);
            if before == Status::InAir && s.status == Status::InWater {
                landed = Some((s.y, s.vy));
                break;
            }
        }
        let (y, _) = landed.expect("the boat reaches the water");
        // `getWaterLevelAbove` scans up from the row holding the boat's top
        // (y = 1 here, air) and answers that row's y plus its water height,
        // 0 — so 1.0, the water block's top, not its 8/9 surface. The snap
        // returns before gravity, so the tick ends exactly at the target.
        let target = 1.0 - BOAT_HEIGHT + 0.101;
        assert!((y - target).abs() < 1e-9, "snapped to {y}, target {target}");
    }

    /// `controlBoat`: forward thrust is 0.04 along the boat's facing
    /// (`-sin(yaw), cos(yaw)`), turning is a degree a tick of
    /// `deltaRotation`, and the paddles follow the keys. In water the
    /// friction 0.9 caps the speed at 0.04 / (1 - 0.9) = 0.4 a tick.
    #[test]
    fn forward_and_turning_follow_control_boat() {
        let world = Pond {
            water: true,
            ice: false,
        };
        let mut s = BoatState::at(0.5, 0.5, 0.5, 0.0);
        for _ in 0..40 {
            tick(&mut s, BoatInput::default(), &world);
        }
        let z0 = s.z;
        let up = BoatInput {
            up: true,
            ..Default::default()
        };
        for _ in 0..100 {
            tick(&mut s, up, &world);
        }
        assert!(s.z - z0 > 10.0, "yaw 0 faces +z: moved {}", s.z - z0);
        assert!(s.x.abs() - 0.5 < 1e-6);
        // `v = 0.9 v + 0.04` each tick (friction, then thrust) → 0.4.
        assert!(
            (s.vz - 0.4).abs() < 1e-3,
            "terminal speed 0.04 / (1 - 0.9): {}",
            s.vz
        );
        assert_eq!(s.paddles, [true, true]);
        let left = BoatInput {
            left: true,
            ..Default::default()
        };
        let yaw0 = s.yaw;
        tick(&mut s, left, &world);
        assert!(s.yaw < yaw0, "left turns counter-clockwise");
        assert_eq!(s.paddles, [false, true], "left key: the right paddle rows");
    }

    /// On land a boat is a sled whose friction is the ground's, re-read every
    /// tick: `v_n = f v_(n-1) + 0.04`, so after 20 ticks stone (0.6) is at
    /// its terminal 0.1 and ice (0.98) at `0.04 (1 - 0.98^20) / 0.02`.
    #[test]
    fn on_land_friction_comes_from_the_ground() {
        let up = BoatInput {
            up: true,
            ..Default::default()
        };
        let mut stone = BoatState::at(0.5, 0.0, 0.5, 0.0);
        let mut ice = stone;
        let w_stone = Pond {
            water: false,
            ice: false,
        };
        let w_ice = Pond {
            water: false,
            ice: true,
        };
        for _ in 0..20 {
            tick(&mut stone, up, &w_stone);
            tick(&mut ice, up, &w_ice);
        }
        assert_eq!(stone.status, Status::OnLand);
        let closed = |f: f64| 0.04f32 as f64 * (1.0 - f.powi(20)) / (1.0 - f);
        assert!(
            (stone.vz - closed(0.6f32 as f64)).abs() < 1e-6,
            "stone {}",
            stone.vz
        );
        assert!(
            (ice.vz - closed(0.98f32 as f64)).abs() < 1e-6,
            "ice {}",
            ice.vz
        );
    }
}
