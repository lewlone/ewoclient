//! Local-player physics — a port of the vanilla 20 Hz tick from the decompiled
//! 26.2 `LocalPlayer.aiStep` → `LivingEntity.aiStep` → `Player.travel` →
//! `LivingEntity.travel{InAir,InFluid}` → `Entity.move` chain.
//!
//! Vanilla does most of this arithmetic in `float` (block friction, the speed
//! passed to `moveRelative`, the drag factors, the sin/cos table) and widens to
//! `double` only when it meets a velocity. The port keeps those types, because
//! `0.91F` widened is `0.9100000262…`, not `0.91`.
//!
//! Collision is `Entity.collide`: one collider list collected from the
//! movement-expanded box by `BlockCollisions` (which also scans a one-block
//! shell for shapes taller than a block — fences, walls), then
//! `collideWithShapes` in `Direction.axisStepOrder` (Y, then the larger
//! horizontal axis last), then the step-up retry over the candidate heights the
//! colliders offer, only when on the ground or landing this tick.
//!
//! Covered: walking/sprinting/sneaking input (`modifyInput`, square-movement
//! correction), jumping with `noJumpDelay`, per-block friction / speed / jump
//! factors, bouncing (slime, beds), climbing (`#climbable`, open trapdoors over
//! ladders, sneak-to-hold), `makeStuckInBlock` (cobweb, sweet berry bush,
//! powder snow), bubble columns (`BubbleColumnBlock.entityInside`), honey wall
//! sliding (`HoneyBlock.entityInside`), the sneak edge guard, water and lava
//! travel with fluid heights, jump-to-swim and fluid currents (`getFlow`),
//! flight (`Player.travel`'s `abilities.flying` arm), no-clip, the
//! unloaded-chunk `-0.1` fall, and the movement attributes.
//!
//! Not covered: swimming pose and the
//! swim-sprint pitch steering, the crouching/swimming bounding boxes (the box
//! is always 0.6 × 1.8), elytra, levitation from blocks, entity colliders
//! (boats, shulkers), powder-snow walking with leather boots.

use rewo_data::block_physics::{flags, BlockPhysics, PhysFluid, Stuck};

use crate::abilities::Abilities;
use crate::border::BorderCollision;
use crate::lightmap::{mth_cos, mth_sin};

/// Player collision box: 0.6 × 1.8 (eye height 1.62).
pub const PLAYER_HALF_WIDTH: f64 = 0.3;
pub const PLAYER_HEIGHT: f64 = 1.8;
pub const EYE_HEIGHT: f64 = 1.62;

/// `Player.getFlyingSpeed()`'s non-flying arm — `0.02F`, widened.
pub const AIR_SPEED: f64 = 0.02f32 as f64;
/// The sprinting counterpart, `0.025999999F` (**not** a doubling).
pub const AIR_SPEED_SPRINT: f64 = 0.025999999f32 as f64;
/// `LivingEntity.MIN_MOVEMENT_DISTANCE`.
const MIN_MOVEMENT_DISTANCE: f64 = 0.003;
/// `AttributeModifier` `SPEED_MODIFIER_SPRINTING`: `ADD_MULTIPLIED_TOTAL 0.3`.
const SPRINT_MODIFIER: f64 = 0.3;
/// `Entity.getFluidJumpThreshold()` for an eye height ≥ 0.4.
const FLUID_JUMP_THRESHOLD: f64 = 0.4;
const EPS: f64 = 1.0e-7;

/// Per-tick input, vanilla conventions: forward +1 = W, strafe +1 = left.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TickInput {
    pub forward: f32,
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

/// The local player's movement attributes and effects. `Default` is a vanilla
/// player with no modifiers: the values `Player.createAttributes` registers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveAttributes {
    /// `movement_speed` **without** the sprint modifier (the tick applies it
    /// from `TickInput::sprint`, as `LivingEntity.setSprinting` would).
    pub movement_speed: f64,
    pub jump_strength: f64,
    pub step_height: f64,
    pub gravity: f64,
    pub sneaking_speed: f64,
    pub water_movement_efficiency: f64,
    pub friction_modifier: f64,
    pub air_drag_modifier: f64,
    /// `JUMP_BOOST` amplifier, if the effect is active.
    pub jump_boost: Option<i32>,
    pub slow_falling: bool,
    pub dolphins_grace: bool,
    /// `LEVITATION` amplifier, if active.
    pub levitation: Option<i32>,
}

impl Default for MoveAttributes {
    fn default() -> Self {
        Self {
            movement_speed: 0.1f32 as f64,
            jump_strength: 0.42f32 as f64,
            step_height: 0.6,
            gravity: 0.08,
            sneaking_speed: 0.3,
            water_movement_efficiency: 0.0,
            friction_modifier: 1.0,
            air_drag_modifier: 1.0,
            jump_boost: None,
            slow_falling: false,
            dolphins_grace: false,
            levitation: None,
        }
    }
}

/// What the physics reads from the world.
pub trait PhysicsWorld {
    /// The block's collision boxes in block-local `0..1` (they may extend
    /// past it: a fence post reaches 1.5). Empty for no collision.
    fn collision(&self, x: i32, y: i32, z: i32) -> &[[f32; 6]];
    /// The block's movement behaviour.
    fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics;
    /// `Level.hasChunkAt` — false for an unloaded column.
    fn has_chunk(&self, x: i32, z: i32) -> bool;
    /// The dimension's lowest y (`Level.getMinY`).
    fn min_y(&self) -> i32;
}

/// A world that is only collision shapes: every block behaves like stone
/// (friction 0.6, no fluid, not climbable) and every chunk is loaded.
pub struct ShapesOnly<'a, 's> {
    pub shapes: &'a dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
}

impl<'s> PhysicsWorld for ShapesOnly<'_, 's> {
    fn collision(&self, x: i32, y: i32, z: i32) -> &[[f32; 6]] {
        (self.shapes)(x, y, z)
    }
    fn block(&self, _x: i32, _y: i32, _z: i32) -> BlockPhysics {
        BlockPhysics::default()
    }
    fn has_chunk(&self, _x: i32, _z: i32) -> bool {
        true
    }
    fn min_y(&self) -> i32 {
        i32::MIN
    }
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
    pub vertical_collision: bool,
    /// `Entity.fallDistance`.
    pub fall_distance: f64,
    /// `Entity.mainSupportingBlockPos`.
    pub supporting_block: Option<[i32; 3]>,
    /// `Entity.onGroundNoBlocks`.
    pub on_ground_no_blocks: bool,
    /// `Entity.stuckSpeedMultiplier` — consumed by the next tick's move.
    pub stuck_multiplier: [f64; 3],
    /// `LivingEntity.noJumpDelay`.
    pub no_jump_delay: i32,
    /// `EntityFluidInteraction` heights (0 = not in that fluid).
    pub water_height: f64,
    pub lava_height: f64,
    /// `isEyeInFluid(WATER)` as of this tick's fluid update.
    pub eye_in_water: bool,
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
            vertical_collision: false,
            fall_distance: 0.0,
            supporting_block: None,
            on_ground_no_blocks: false,
            stuck_multiplier: [0.0; 3],
            no_jump_delay: 0,
            water_height: 0.0,
            lava_height: 0.0,
            eye_in_water: false,
        }
    }

    pub fn eye_y(&self) -> f64 {
        self.y + EYE_HEIGHT
    }

    pub fn in_water(&self) -> bool {
        self.water_height > 0.0
    }

    pub fn in_lava(&self) -> bool {
        self.lava_height > 0.0
    }

    fn aabb(&self) -> Aabb {
        [
            self.x - PLAYER_HALF_WIDTH,
            self.y,
            self.z - PLAYER_HALF_WIDTH,
            self.x + PLAYER_HALF_WIDTH,
            self.y + PLAYER_HEIGHT,
            self.z + PLAYER_HALF_WIDTH,
        ]
    }

    fn block_pos(&self) -> [i32; 3] {
        [self.x.floor() as i32, self.y.floor() as i32, self.z.floor() as i32]
    }
}

/// One vanilla tick, walking, against collision shapes only (every block
/// behaves like stone). Kept for callers and gates that predate block
/// behaviour; [`tick_env`] is the full form.
pub fn tick<'s>(
    state: &mut PlayerState,
    input: &TickInput,
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) {
    tick_with(state, input, &Abilities::default(), false, None, shapes);
}

/// [`tick`] with abilities (flight), no-clip and the world border.
pub fn tick_with<'s>(
    state: &mut PlayerState,
    input: &TickInput,
    abilities: &Abilities,
    no_clip: bool,
    border: Option<BorderCollision>,
    shapes: &dyn Fn(i32, i32, i32) -> &'s [[f32; 6]],
) {
    let world = ShapesOnly { shapes };
    tick_env(
        state,
        input,
        abilities,
        no_clip,
        border,
        &MoveAttributes::default(),
        &world,
    );
}

/// One vanilla tick in any movement mode, against a full [`PhysicsWorld`].
///
/// `abilities.flying` selects flight; `no_clip` is `Entity.noPhysics`
/// (`Player.tick` sets it from `isSpectator()`). `border` is the world
/// border's collision extent, a collider like the blocks. The flight toggle
/// and vertical flight impulse (`LocalPlayer.aiStep`'s prologue) are the
/// caller's (`abilities::FlightControl`), and run before this.
pub fn tick_env(
    state: &mut PlayerState,
    input: &TickInput,
    abilities: &Abilities,
    no_clip: bool,
    border: Option<BorderCollision>,
    attrs: &MoveAttributes,
    world: &dyn PhysicsWorld,
) {
    let flying = abilities.flying;
    let affected_by_fluids = !flying;
    let mut ctx = Ctx {
        world,
        attrs,
        border,
        input,
        no_clip,
        flying,
        crouching: input.sneak && !flying,
        air_speed: abilities.air_move_speed(input.sprint, flying) as f32,
    };

    // -- Entity.baseTick: fluid interaction ---------------------------------
    update_fluids(state, world, !flying);
    if state.in_water() {
        state.fall_distance = 0.0;
    }
    if state.in_lava() {
        state.fall_distance *= 0.5;
    }

    // -- LocalPlayer.aiStep: sneaking sinks in water --------------------------
    if state.in_water() && input.sneak && affected_by_fluids {
        state.vy += (-0.04f32) as f64;
    }

    // -- LivingEntity.aiStep -------------------------------------------------
    if state.no_jump_delay > 0 {
        state.no_jump_delay -= 1;
    }
    // Min-movement clamp — the first statement, and a *joint* horizontal test
    // for a player (`horizontalDistanceSqr() < 9.0E-6`).
    if state.vx * state.vx + state.vz * state.vz < 9.0e-6 {
        state.vx = 0.0;
        state.vz = 0.0;
    }
    if state.vy.abs() < MIN_MOVEMENT_DISTANCE {
        state.vy = 0.0;
    }
    let (xxa, zza) = modify_input(input, ctx.crouching, attrs);

    if input.jump && affected_by_fluids {
        let in_lava = state.in_lava();
        let fluid_height = if in_lava { state.lava_height } else { state.water_height };
        let in_water_with_height = state.in_water() && fluid_height > 0.0;
        if !in_water_with_height || state.on_ground && !(fluid_height > FLUID_JUMP_THRESHOLD) {
            if !in_lava || state.on_ground && state.lava_height <= FLUID_JUMP_THRESHOLD {
                if (state.on_ground
                    || in_water_with_height && fluid_height <= FLUID_JUMP_THRESHOLD)
                    && state.no_jump_delay == 0
                {
                    jump_from_ground(state, &ctx);
                    state.no_jump_delay = 10;
                }
            } else {
                state.vy += 0.04f32 as f64;
            }
        } else {
            state.vy += 0.04f32 as f64;
        }
    } else {
        state.no_jump_delay = 0;
    }

    // -- Player.travel -------------------------------------------------------
    if flying {
        let original_vy = state.vy;
        travel(state, &mut ctx, xxa, zza);
        state.vy = original_vy * crate::abilities::FLYING_VERTICAL_DECAY;
    } else {
        travel(state, &mut ctx, xxa, zza);
    }

    // -- applyEffectsFromBlocks: entityInside -------------------------------
    if !no_clip {
        apply_stuck(state, world, flying);
    }
}

struct Ctx<'a> {
    world: &'a dyn PhysicsWorld,
    attrs: &'a MoveAttributes,
    border: Option<BorderCollision>,
    input: &'a TickInput,
    no_clip: bool,
    flying: bool,
    crouching: bool,
    /// `Player.getFlyingSpeed()` (the airborne `moveRelative` amount).
    air_speed: f32,
}

/// `LocalPlayer.modifyInput`: ×0.98, ×`sneaking_speed` while crouching, then
/// `modifyInputSpeedForSquareMovement` — a diagonal is stretched toward the
/// unit square's corner (capped at length 1).
fn modify_input(input: &TickInput, crouching: bool, attrs: &MoveAttributes) -> (f32, f32) {
    let (mut x, mut y) = (input.strafe, input.forward);
    if x * x + y * y == 0.0 {
        return (x, y);
    }
    x *= 0.98;
    y *= 0.98;
    if crouching {
        let f = attrs.sneaking_speed as f32;
        x *= f;
        y *= f;
    }
    let length = (x * x + y * y).sqrt();
    if length <= 0.0 {
        return (x, y);
    }
    let (dx, dy) = (x * (1.0 / length), y * (1.0 / length));
    let (ax, ay) = (dx.abs(), dy.abs());
    let tan = if ay > ax { ax / ay } else { ay / ax };
    let to_square = (1.0 + tan * tan).sqrt();
    let modified = (length * to_square).min(1.0);
    (dx * modified, dy * modified)
}

/// `Entity.moveRelative` / `getInputVector` with the `Mth` sine table.
fn move_relative(state: &mut PlayerState, speed: f32, xxa: f32, zza: f32) {
    let (mut mx, mut mz) = (xxa as f64, zza as f64);
    let len = mx * mx + mz * mz;
    if len < 1.0e-7 {
        return;
    }
    if len > 1.0 {
        let l = len.sqrt();
        mx /= l;
        mz /= l;
    }
    mx *= speed as f64;
    mz *= speed as f64;
    let angle = state.yaw * (std::f64::consts::PI / 180.0) as f32;
    let (sin, cos) = (mth_sin(angle) as f64, mth_cos(angle) as f64);
    state.vx += mx * cos - mz * sin;
    state.vz += mz * cos + mx * sin;
}

/// `Player.getSpeed()`: the movement-speed attribute with the sprint
/// modifier, as a float.
fn speed(ctx: &Ctx) -> f32 {
    let mult = if ctx.input.sprint { 1.0 + SPRINT_MODIFIER } else { 1.0 };
    (ctx.attrs.movement_speed * mult) as f32
}

fn effective_gravity(state: &PlayerState, attrs: &MoveAttributes) -> f64 {
    if state.vy <= 0.0 && attrs.slow_falling {
        attrs.gravity.min(0.01)
    } else {
        attrs.gravity
    }
}

/// `LivingEntity.computeModifiedFriction`.
fn modified_friction(friction: f32, modifier: f32) -> f32 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

/// `LivingEntity.jumpFromGround`.
fn jump_from_ground(state: &mut PlayerState, ctx: &Ctx) {
    let boost = ctx.attrs.jump_boost.map_or(0.0, |a| 0.1 * (a as f32 + 1.0));
    let power = ctx.attrs.jump_strength as f32 * 1.0 * block_jump_factor(state, ctx.world) + boost;
    if power <= 1.0e-5 {
        return;
    }
    state.vy = state.vy.max(power as f64);
    if ctx.input.sprint {
        let angle = state.yaw * (std::f64::consts::PI / 180.0) as f32;
        state.vx += -mth_sin(angle) as f64 * 0.2;
        state.vz += mth_cos(angle) as f64 * 0.2;
    }
}

/// `LivingEntity.travel` (fall flying not modelled).
fn travel(state: &mut PlayerState, ctx: &mut Ctx, xxa: f32, zza: f32) {
    if (state.in_water() || state.in_lava()) && !ctx.flying {
        travel_in_fluid(state, ctx, xxa, zza);
    } else {
        travel_in_air(state, ctx, xxa, zza);
    }
}

fn travel_in_air(state: &mut PlayerState, ctx: &mut Ctx, xxa: f32, zza: f32) {
    let below = on_pos(state, ctx.world, 0.500001);
    let block_friction = if state.on_ground {
        modified_friction(
            ctx.world.block(below[0], below[1], below[2]).friction,
            ctx.attrs.friction_modifier as f32,
        )
    } else {
        1.0
    };
    // handleRelativeFrictionAndCalculateMovement
    let accel = if state.on_ground {
        if block_friction as f64 > 0.6 {
            speed(ctx) * (0.21600002f32 / (block_friction * block_friction * block_friction))
        } else {
            speed(ctx)
        }
    } else {
        ctx.air_speed
    };
    move_relative(state, accel, xxa, zza);
    handle_on_climbable(state, ctx);
    let (dx, dy, dz) = (state.vx, state.vy, state.vz);
    do_move(state, ctx, [dx, dy, dz]);
    let mut vy = state.vy;
    if (state.horizontal_collision || ctx.input.jump) && on_climbable(state, ctx) {
        vy = 0.2;
    }

    let mut movement_y = vy;
    if let Some(amp) = ctx.attrs.levitation {
        movement_y += (0.05 * (amp + 1) as f64 - vy) * 0.2;
    } else if ctx.world.has_chunk(below[0], below[2]) {
        movement_y -= effective_gravity(state, ctx.attrs);
    } else if state.y > ctx.world.min_y() as f64 {
        movement_y = -0.1;
    } else {
        movement_y = 0.0;
    }
    let air_drag_mod = ctx.attrs.air_drag_modifier as f32;
    let air_drag = modified_friction(0.91, air_drag_mod);
    let friction = block_friction * air_drag;
    let vertical = modified_friction(0.98, air_drag_mod);
    state.vx *= friction as f64;
    state.vy = movement_y * vertical as f64;
    state.vz *= friction as f64;
}

fn travel_in_fluid(state: &mut PlayerState, ctx: &mut Ctx, xxa: f32, zza: f32) {
    let is_falling = state.vy <= 0.0;
    let old_y = state.y;
    let base_gravity = effective_gravity(state, ctx.attrs);
    if state.in_water() {
        let mut slow_down: f32 = if ctx.input.sprint { 0.9 } else { 0.8 };
        let mut speed_f: f32 = 0.02;
        let mut walker = ctx.attrs.water_movement_efficiency as f32;
        if !state.on_ground {
            walker *= 0.5;
        }
        if walker > 0.0 {
            slow_down += (0.54600006 - slow_down) * walker;
            speed_f += (speed(ctx) - speed_f) * walker;
        }
        if ctx.attrs.dolphins_grace {
            slow_down = 0.96;
        }
        move_relative(state, speed_f, xxa, zza);
        let d = [state.vx, state.vy, state.vz];
        do_move(state, ctx, d);
        let mut m = [state.vx, state.vy, state.vz];
        if state.horizontal_collision && on_climbable(state, ctx) {
            m[1] = 0.2;
        }
        m[0] *= slow_down as f64;
        m[1] *= 0.8f32 as f64;
        m[2] *= slow_down as f64;
        m[1] = fluid_falling_adjusted(ctx, base_gravity, is_falling, m[1]);
        state.vx = m[0];
        state.vy = m[1];
        state.vz = m[2];
    } else {
        move_relative(state, 0.02, xxa, zza);
        let d = [state.vx, state.vy, state.vz];
        do_move(state, ctx, d);
        if state.lava_height <= FLUID_JUMP_THRESHOLD {
            state.vx *= 0.5;
            state.vy *= 0.8f32 as f64;
            state.vz *= 0.5;
            state.vy = fluid_falling_adjusted(ctx, base_gravity, is_falling, state.vy);
        } else {
            state.vx *= 0.5;
            state.vy *= 0.5;
            state.vz *= 0.5;
        }
        if base_gravity != 0.0 {
            state.vy += -base_gravity / 4.0;
        }
    }
    // jumpOutOfFluid
    if state.horizontal_collision {
        let probe = offset(&state.aabb(), [state.vx, state.vy + 0.6f32 as f64 - state.y + old_y, state.vz]);
        if no_collision(&probe, ctx.world) && !contains_liquid(&probe, ctx.world) {
            state.vy = 0.3f32 as f64;
        }
    }
}

/// `LivingEntity.getFluidFallingAdjustedMovement` (the y component).
fn fluid_falling_adjusted(ctx: &Ctx, base_gravity: f64, is_falling: bool, vy: f64) -> f64 {
    if base_gravity != 0.0 && !ctx.input.sprint {
        if is_falling && (vy - 0.005).abs() >= 0.003 && (vy - base_gravity / 16.0).abs() < 0.003 {
            -0.003
        } else {
            vy - base_gravity / 16.0
        }
    } else {
        vy
    }
}

/// `LivingEntity.onClimbable`.
fn on_climbable(state: &PlayerState, ctx: &Ctx) -> bool {
    if ctx.no_clip {
        return false;
    }
    let [x, y, z] = state.block_pos();
    let here = ctx.world.block(x, y, z);
    if here.has(flags::CLIMBABLE) {
        return true;
    }
    if here.has(flags::OPEN_TRAPDOOR) {
        let below = ctx.world.block(x, y - 1, z);
        return below.has(flags::LADDER) && below.facing == here.facing;
    }
    false
}

/// `LivingEntity.handleOnClimbable`.
fn handle_on_climbable(state: &mut PlayerState, ctx: &Ctx) {
    if !on_climbable(state, ctx) {
        return;
    }
    state.fall_distance = 0.0;
    let max = 0.15f32 as f64;
    state.vx = state.vx.clamp(-max, max);
    state.vz = state.vz.clamp(-max, max);
    let mut yd = state.vy.max(-max);
    let [x, y, z] = state.block_pos();
    if yd < 0.0 && !ctx.world.block(x, y, z).has(flags::SCAFFOLDING) && ctx.input.sneak {
        yd = 0.0;
    }
    state.vy = yd;
}

/// `Entity.getOnPos(offset)`.
fn on_pos(state: &PlayerState, world: &dyn PhysicsWorld, offset: f32) -> [i32; 3] {
    if let Some(sp) = state.supporting_block {
        if !(offset > 1.0e-5) {
            return sp;
        }
        let below = world.block(sp[0], sp[1], sp[2]);
        let keep = (offset <= 0.5 && below.has(flags::FENCE))
            || below.has(flags::WALL)
            || below.has(flags::FENCE_GATE);
        if keep {
            sp
        } else {
            [sp[0], (state.y - offset as f64).floor() as i32, sp[2]]
        }
    } else {
        [
            state.x.floor() as i32,
            (state.y - offset as f64).floor() as i32,
            state.z.floor() as i32,
        ]
    }
}

/// `Entity.getBlockJumpFactor`.
fn block_jump_factor(state: &PlayerState, world: &dyn PhysicsWorld) -> f32 {
    let [x, y, z] = state.block_pos();
    let here = world.block(x, y, z).jump_factor;
    if here == 1.0 {
        let b = on_pos(state, world, 0.500001);
        world.block(b[0], b[1], b[2]).jump_factor
    } else {
        here
    }
}

/// `Entity.getBlockSpeedFactor` (`Player`: 1.0 while flying).
fn block_speed_factor(state: &PlayerState, ctx: &Ctx) -> f32 {
    if ctx.flying {
        return 1.0;
    }
    let [x, y, z] = state.block_pos();
    let here = ctx.world.block(x, y, z);
    if !here.has(flags::WATER_BLOCK) && here.speed_factor == 1.0 {
        let b = on_pos(state, ctx.world, 0.500001);
        ctx.world.block(b[0], b[1], b[2]).speed_factor
    } else {
        here.speed_factor
    }
}

/// `Mth.equal(double, double)`: `|b - a| < 1.0E-5F`.
fn mth_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5f32 as f64
}

/// `Entity.move(MoverType.SELF, delta)`.
fn do_move(state: &mut PlayerState, ctx: &Ctx, mut delta: [f64; 3]) {
    if ctx.no_clip {
        state.x += delta[0];
        state.y += delta[1];
        state.z += delta[2];
        state.horizontal_collision = false;
        state.vertical_collision = false;
        // `Player.tick` forces a spectator off the ground.
        state.on_ground = false;
        return;
    }
    let m = state.stuck_multiplier;
    if m[0] * m[0] + m[1] * m[1] + m[2] * m[2] > 1.0e-7 {
        delta = [delta[0] * m[0], delta[1] * m[1], delta[2] * m[2]];
        state.stuck_multiplier = [0.0; 3];
        state.vx = 0.0;
        state.vy = 0.0;
        state.vz = 0.0;
    }
    delta = back_off_from_edge(state, ctx, delta);
    let movement = collide(state, ctx, delta);
    let moved_sq = len_sq(movement);
    if moved_sq > 1.0e-7 || len_sq(delta) - moved_sq < 1.0e-7 {
        state.x += movement[0];
        state.y += movement[1];
        state.z += movement[2];
    }
    let x_coll = !mth_equal(delta[0], movement[0]);
    let z_coll = !mth_equal(delta[2], movement[2]);
    state.horizontal_collision = x_coll || z_coll;
    let moved_vertically = delta[1].abs() > 0.0;
    // The local player is always "local-instance authoritative".
    state.vertical_collision = delta[1] != movement[1];
    let below = state.vertical_collision && delta[1] < 0.0;
    state.on_ground = below;
    check_supporting_block(state, ctx.world, below, Some(movement));

    // `LivingEntity.checkFallDamage` refreshes the fluids first (landing in
    // water this tick counts), then `Entity.checkFallDamage`. Vanilla's
    // `updateFluidInteraction` pushes the currents here too, so a move that
    // lands the player in a flow takes the impulse mid-tick.
    if !state.in_water() {
        update_fluids(state, ctx.world, !ctx.flying);
    }
    if !state.in_water() && movement[1] < 0.0 {
        state.fall_distance -= movement[1] as f32 as f64;
    }
    if state.on_ground {
        state.fall_distance = 0.0;
    }

    if moved_vertically && state.vertical_collision || state.horizontal_collision {
        restitute(state, ctx, x_coll, z_coll, movement);
    }
    let f = block_speed_factor(state, ctx) as f64;
    state.vx *= f;
    state.vz *= f;
}

/// `Entity.restituteMovementAfterCollisions` for a player (no entity
/// bounciness; a bounce comes only from the block landed on).
fn restitute(state: &mut PlayerState, ctx: &Ctx, x_coll: bool, z_coll: bool, movement: [f64; 3]) {
    let suppressing = ctx.input.sneak;
    let mut restitution = 0.0f64;
    if x_coll {
        state.vx = -state.vx * restitution;
    }
    if z_coll {
        state.vz = -state.vz * restitution;
    }
    if state.vertical_collision {
        let gravity = effective_gravity(state, ctx.attrs);
        // `verticalCollisionBelow` — `on_ground` was just set from it.
        if state.on_ground {
            let e = on_pos(state, ctx.world, 0.2);
            let effect = ctx.world.block(e[0], e[1], e[2]);
            restitution = if !(-state.vy < gravity)
                && !suppressing
                && !effect.has(flags::SUPPRESSES_BOUNCE)
            {
                restitution.max(effect.bounce as f64)
            } else {
                0.0
            };
        }
        let (comp, drag) = if restitution > 0.0 {
            let portion = movement[1] / state.vy;
            let air_drag = modified_friction(0.98, ctx.attrs.air_drag_modifier as f32) as f64;
            (portion * gravity, 1.0 + portion * (air_drag - 1.0))
        } else {
            (0.0, 1.0)
        };
        state.vy = (comp - state.vy) * drag * restitution;
    }
}

/// `Player.maybeBackOffFromEdge`: while sneaking on the ground, shorten each
/// horizontal step (0.05 at a time) until it would not drop more than a step.
fn back_off_from_edge(state: &PlayerState, ctx: &Ctx, delta: [f64; 3]) -> [f64; 3] {
    let max_down = ctx.attrs.step_height as f32 as f64;
    let above_ground = || {
        state.on_ground
            || state.fall_distance < max_down
                && !can_fall_at_least(state, ctx.world, 0.0, 0.0, max_down - state.fall_distance)
    };
    if ctx.flying || delta[1] > 0.0 || !ctx.input.sneak || !above_ground() {
        return delta;
    }
    let (mut dx, mut dz) = (delta[0], delta[2]);
    let (sx, sz) = (dx.signum() * 0.05, dz.signum() * 0.05);
    let fall = |dx: f64, dz: f64| can_fall_at_least(state, ctx.world, dx, dz, max_down);
    while dx != 0.0 && fall(dx, 0.0) {
        if dx.abs() <= 0.05 {
            dx = 0.0;
            break;
        }
        dx -= sx;
    }
    while dz != 0.0 && fall(0.0, dz) {
        if dz.abs() <= 0.05 {
            dz = 0.0;
            break;
        }
        dz -= sz;
    }
    while dx != 0.0 && dz != 0.0 && fall(dx, dz) {
        if dx.abs() <= 0.05 {
            dx = 0.0;
        } else {
            dx -= sx;
        }
        if dz.abs() <= 0.05 {
            dz = 0.0;
        } else {
            dz -= sz;
        }
    }
    [dx, delta[1], dz]
}

fn can_fall_at_least(state: &PlayerState, world: &dyn PhysicsWorld, dx: f64, dz: f64, min: f64) -> bool {
    let b = state.aabb();
    let probe = [
        b[0] + EPS + dx,
        b[1] - min - EPS,
        b[2] + EPS + dz,
        b[3] - EPS + dx,
        b[1],
        b[5] - EPS + dz,
    ];
    no_collision(&probe, world)
}

/// `Entity.checkSupportingBlock`.
fn check_supporting_block(state: &mut PlayerState, world: &dyn PhysicsWorld, on_ground: bool, movement: Option<[f64; 3]>) {
    if on_ground {
        let b = state.aabb();
        let test = [b[0], b[1] - 1.0e-6, b[2], b[3], b[1], b[5]];
        let mut found = find_supporting_block(state, world, &test);
        if found.is_some() || state.on_ground_no_blocks {
            state.supporting_block = found;
        } else if let Some(m) = movement {
            found = find_supporting_block(state, world, &offset(&test, [-m[0], 0.0, -m[2]]));
            state.supporting_block = found;
        }
        state.on_ground_no_blocks = found.is_none();
    } else {
        state.on_ground_no_blocks = false;
        state.supporting_block = None;
    }
}

/// `CollisionGetter.findSupportingBlock`: the colliding block whose centre is
/// nearest the entity's position; ties go to the greatest `BlockPos`
/// (`Vec3i.compareTo`: y, then z, then x).
fn find_supporting_block(state: &PlayerState, world: &dyn PhysicsWorld, test: &Aabb) -> Option<[i32; 3]> {
    let mut best: Option<([i32; 3], f64)> = None;
    for_each_collider(test, world, |pos, _| {
        let d = |c: i32, p: f64| c as f64 + 0.5 - p;
        let dist = d(pos[0], state.x).powi(2) + d(pos[1], state.y).powi(2) + d(pos[2], state.z).powi(2);
        let key = |p: [i32; 3]| (p[1], p[2], p[0]);
        let better = match best {
            None => true,
            Some((bp, bd)) => dist < bd || dist == bd && key(bp) < key(pos),
        };
        if better {
            best = Some((pos, dist));
        }
    });
    best.map(|b| b.0)
}

/// `applyEffectsFromBlocks` → `entityInside` over the blocks the final
/// (deflated) box touches: `makeStuckInBlock` (cobweb, sweet berry bush,
/// powder snow), `BubbleColumnBlock.entityInside` and `HoneyBlock.entityInside`.
/// Every block this loop visits overlaps the deflated box, which is exactly
/// vanilla's `isPrecise`, so the bubble column's precise-only gate is free.
fn apply_stuck(state: &mut PlayerState, world: &dyn PhysicsWorld, flying: bool) {
    let b = deflate(&state.aabb(), 1.0e-5f32 as f64);
    let feet = state.block_pos();
    let feet_powder = world.block(feet[0], feet[1], feet[2]).has(flags::POWDER_SNOW);
    for y in b[1].floor() as i32..=b[4].floor() as i32 {
        for z in b[2].floor() as i32..=b[5].floor() as i32 {
            for x in b[0].floor() as i32..=b[3].floor() as i32 {
                let block = world.block(x, y, z);
                let applies = match block.stuck {
                    Stuck::None => false,
                    Stuck::Web | Stuck::SweetBerryBush => true,
                    Stuck::PowderSnow => feet_powder,
                };
                if let (true, Some(m)) = (applies, block.stuck.multiplier()) {
                    state.fall_distance = 0.0;
                    state.stuck_multiplier = m;
                }
                if block.has(flags::BUBBLE_COLUMN_UP | flags::BUBBLE_COLUMN_DOWN) && !flying {
                    // `Player.onAboveBubbleColumn` / `onInsideBubbleColumn`
                    // pass everything through only while not flying.
                    let down = block.has(flags::BUBBLE_COLUMN_DOWN);
                    bubble_column_effect(state, world, x, y, z, down);
                } else if block.has(flags::HONEY) && honey_sliding_down(x, y, z, state) {
                    // The slide achievement and its sound/particles are not movement.
                    honey_slide(state);
                }
            }
        }
    }
}

/// `BubbleColumnBlock.entityInside` → `Entity.handleOnAboveBubbleColumn` /
/// `handleOnInsideBubbleColumn`. The "above" arm is vanilla's `nothingAbove`
/// case: the block over the column has neither a collision shape nor a fluid
/// (`stateAbove.getCollisionShape(...).isEmpty() &&
/// stateAbove.getFluidState().isEmpty()` — vanilla passes the column's own
/// `pos` to `getCollisionShape`, which no block reads).
fn bubble_column_effect(
    state: &mut PlayerState, world: &dyn PhysicsWorld, x: i32, y: i32, z: i32, drag_down: bool,
) {
    let above = world.block(x, y + 1, z);
    let nothing_above = world.collision(x, y + 1, z).is_empty() && above.fluid == PhysFluid::None;
    if nothing_above {
        state.vy = if drag_down {
            (state.vy - 0.03).max(-0.9)
        } else {
            (state.vy + 0.1).min(1.8)
        };
    } else {
        state.vy = if drag_down {
            (state.vy - 0.03).max(-0.3)
        } else {
            (state.vy + 0.06).min(0.7)
        };
        state.fall_distance = 0.0;
    }
}

/// `HoneyBlock.SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST`.
const SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST: f64 = 0.13;
/// `HoneyBlock.MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING`.
const MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING: f64 = 0.08;
/// `HoneyBlock.THROTTLE_SLIDE_SPEED_TO`.
const THROTTLE_SLIDE_SPEED_TO: f64 = 0.05;

/// `HoneyBlock.getOldDeltaY`: the tick's gravity and `0.98F` drag undone.
fn honey_old_delta_y(delta_y: f64) -> f64 {
    delta_y / (0.98f32 as f64) + 0.08
}

/// `HoneyBlock.getNewDeltaY`.
fn honey_new_delta_y(delta_y: f64) -> f64 {
    (delta_y - 0.08) * (0.98f32 as f64)
}

/// `HoneyBlock.isSlidingDown`: beside the block's side (the centre distance
/// passes `0.4375 + getBbWidth() / 2.0F`, i.e. the player overlaps the cell
/// but not its 14/16 shape), at or below the 15/16 top, airborne and falling.
fn honey_sliding_down(x: i32, y: i32, z: i32, state: &PlayerState) -> bool {
    if state.on_ground {
        return false;
    }
    if state.y > y as f64 + 0.9375 - 1.0e-7 {
        return false;
    }
    if honey_old_delta_y(state.vy) >= -MIN_FALL_SPEED_TO_BE_CONSIDERED_SLIDING {
        return false;
    }
    let dx = (x as f64 + 0.5 - state.x).abs();
    let dz = (z as f64 + 0.5 - state.z).abs();
    // `entity.getBbWidth() / 2.0F` — the 0.6F box width, halved in float.
    let bb_width = (PLAYER_HALF_WIDTH * 2.0) as f32;
    let overlap_distance = 0.4375 + (bb_width / 2.0f32) as f64;
    dx + 1.0e-7 > overlap_distance || dz + 1.0e-7 > overlap_distance
}

/// `HoneyBlock.doSlideMovement`: the fall is throttled to
/// `getNewDeltaY(-THROTTLE_SLIDE_SPEED_TO)` whatever it was, horizontal
/// velocity scales with it past the slide threshold, fall distance resets.
fn honey_slide(state: &mut PlayerState) {
    let old_delta_y = honey_old_delta_y(state.vy);
    if old_delta_y < -SLIDE_STARTS_WHEN_VERTICAL_SPEED_IS_AT_LEAST {
        let horizontal_reduction_factor = -THROTTLE_SLIDE_SPEED_TO / old_delta_y;
        state.vx *= horizontal_reduction_factor;
        state.vz *= horizontal_reduction_factor;
    }
    state.vy = honey_new_delta_y(-THROTTLE_SLIDE_SPEED_TO);
    state.fall_distance = 0.0;
}

/// `EntityFluidInteraction.update` plus the current push that
/// `Entity.updateFluidInteraction` adds afterwards: `FlowingFluid.getFlow` per
/// sampled cell, then `Tracker.applyCurrentTo` once per fluid kind.
/// `pushed_by_fluid` is `isPushedByFluid()` (for the player, `!abilities.flying`)
/// — vanilla samples no current at all for a fluid that does not push.
fn update_fluids(state: &mut PlayerState, world: &dyn PhysicsWorld, pushed_by_fluid: bool) {
    state.water_height = 0.0;
    state.lava_height = 0.0;
    state.eye_in_water = false;
    let bx = deflate(&state.aabb(), 0.001);
    let (x0, y0, z0) = (bx[0].floor() as i32, bx[1].floor() as i32, bx[2].floor() as i32);
    let (x1, y1, z1) = (
        bx[3].ceil() as i32 - 1,
        bx[4].ceil() as i32 - 1,
        bx[5].ceil() as i32 - 1,
    );
    // `hasFluidAndLoaded`: an unloaded chunk in the ±1 ring skips the update.
    for cz in (z0 - 1) >> 4..=(z1 + 1) >> 4 {
        for cx in (x0 - 1) >> 4..=(x1 + 1) >> 4 {
            if !world.has_chunk(cx << 4, cz << 4) {
                return;
            }
        }
    }
    let entity_y = state.y;
    let eye_y = state.eye_y();
    let (ex, ez) = (state.x.floor() as i32, state.z.floor() as i32);
    // One `Tracker` per fluid kind; water and lava never share one.
    let (mut current_water, mut current_lava) = (Current::default(), Current::default());
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                let Some((lava, top)) = fluid_top(world, x, y, z) else {
                    continue;
                };
                if top < bx[1] {
                    continue;
                }
                if !lava && x == ex && z == ez && eye_y >= y as f64 && eye_y <= top {
                    state.eye_in_water = true;
                }
                let h = if lava { &mut state.lava_height } else { &mut state.water_height };
                *h = (top - entity_y).max(*h);
                if pushed_by_fluid {
                    // `tracker.accumulateCurrent`, with the shallow-tracker
                    // height scale.
                    let flow = fluid_flow(world, x, y, z, lava);
                    let flow = if *h < 0.4 { scale_vec(flow, *h) } else { flow };
                    if lava { current_lava.accumulate(flow) } else { current_water.accumulate(flow) }
                }
            }
        }
    }
    if pushed_by_fluid {
        // `updateFluidInteraction`: water at 0.014, then lava at its own rate.
        if state.in_water() { current_water.apply(state, WATER_FLOW_SCALE); }
        if state.in_lava() { current_lava.apply(state, LAVA_FLOW_SCALE); }
    }
}

/// A fluid cell's type and absolute top (`FluidState.getHeight`: 1.0 with
/// the same fluid above, else the own height).
fn fluid_top(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32) -> Option<(bool, f64)> {
    let (lava, own) = match world.block(x, y, z).fluid {
        PhysFluid::None => return None,
        PhysFluid::Water { own_height } => (false, own_height),
        PhysFluid::Lava { own_height } => (true, own_height),
    };
    let same_above = matches!(
        (lava, world.block(x, y + 1, z).fluid),
        (false, PhysFluid::Water { .. }) | (true, PhysFluid::Lava { .. })
    );
    let h = if same_above { 1.0 } else { own };
    Some((lava, y as f64 + h as f64))
}

/// `Entity.updateFluidInteraction`'s water push per tick, and its lava push
/// with `EnvironmentAttributes.FAST_LAVA` off (the overworld; `0.007` fast).
/// Rewo's physics has no dimension attributes.
const WATER_FLOW_SCALE: f64 = 0.014;
const LAVA_FLOW_SCALE: f64 = 0.0023333333333333335;
/// `Tracker.applyCurrentTo`'s floor: a weak current on a near-stationary
/// player is normalized up to this length.
const MIN_FLOW_IMPULSE: f64 = 0.0045000000000000005;
/// `Direction.Plane.HORIZONTAL`'s order (north, east, south, west), so the
/// height gradient sums in the order Java does.
const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// `EntityFluidInteraction.Tracker`'s current state for one fluid kind.
#[derive(Clone, Copy, Default)]
struct Current {
    sum: [f64; 3], // `accumulatedCurrent`
    count: i32,    // `currentCount`
}

impl Current {
    /// `Tracker.accumulateCurrent`.
    fn accumulate(&mut self, flow: [f64; 3]) {
        self.sum = add_vec(self.sum, flow);
        self.count += 1;
    }

    /// `Tracker.applyCurrentTo` for a player: the mean of the sampled flows,
    /// not the normalized direction a non-player gets, scaled and added to
    /// the velocity — with the nudge that keeps a weak current moving a player
    /// that is otherwise standing still.
    fn apply(self, state: &mut PlayerState, scale: f64) {
        if self.count == 0 || len_sq(self.sum) < 1.0e-5f32 as f64 {
            return;
        }
        let mut impulse = scale_vec(scale_vec(self.sum, 1.0 / self.count as f64), scale);
        if state.vx.abs() < 0.003 && state.vz.abs() < 0.003 && len_sq(impulse).sqrt() < MIN_FLOW_IMPULSE {
            impulse = scale_vec(normalize_vec(impulse), MIN_FLOW_IMPULSE);
        }
        state.vx += impulse[0];
        state.vy += impulse[1];
        state.vz += impulse[2];
    }
}

/// `FlowingFluid.getFlow` for one fluid cell: the unit vector of its flow —
/// the horizontal own-height gradient, plus `(0, -6, 0)` for a falling column
/// walled in by a solid face. Zero when nothing flows.
fn fluid_flow(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32, lava: bool) -> [f64; 3] {
    let here = world.block(x, y, z);
    let own = fluid_own_height(here.fluid);
    let (mut fx, mut fz) = (0.0, 0.0);
    for (dx, dz) in HORIZONTAL {
        let (nx, nz) = (x + dx, z + dz);
        let neighbour = world.block(nx, y, nz);
        if !affects_flow(lava, neighbour.fluid) {
            continue;
        }
        // The height and distance arithmetic is `float` in Java.
        let mut neighbour_own = fluid_own_height(neighbour.fluid);
        let mut distance = 0.0f32;
        if neighbour_own == 0.0 {
            // An empty neighbour cell is read through into the one below it,
            // unless something that blocks motion is in the way.
            if !blocks_motion(world, nx, y, nz) {
                let below = world.block(nx, y - 1, nz);
                if affects_flow(lava, below.fluid) {
                    neighbour_own = fluid_own_height(below.fluid);
                    if neighbour_own > 0.0 {
                        // `0.8888889F` in the Java — 8/9, a float.
                        distance = own - (neighbour_own - 0.8888889f32);
                    }
                }
            }
        } else if neighbour_own > 0.0 {
            distance = own - neighbour_own;
        }
        if distance != 0.0 {
            fx += (dx as f32 * distance) as f64;
            fz += (dz as f32 * distance) as f64;
        }
    }
    let mut flow = [fx, 0.0, fz];
    if here.fluid_falling
        && HORIZONTAL.iter().any(|&(dx, dz)| {
            solid_face(world, x + dx, y, z + dz, (dx, dz), lava)
                || solid_face(world, x + dx, y + 1, z + dz, (dx, dz), lava)
        })
    {
        flow = normalize_vec(flow);
        flow[1] -= 6.0; // `flow.normalize().add(0.0, -6.0, 0.0)`
    }
    normalize_vec(flow)
}

/// `FluidState.getOwnHeight()` — 0 for `FluidState.isEmpty`.
fn fluid_own_height(fluid: PhysFluid) -> f32 {
    match fluid {
        PhysFluid::None => 0.0,
        PhysFluid::Water { own_height } | PhysFluid::Lava { own_height } => own_height,
    }
}

/// `FlowingFluid.affectsFlow`: an empty neighbour, or the same fluid
/// (`isSame` — water and lava never mix).
fn affects_flow(lava: bool, fluid: PhysFluid) -> bool {
    matches!(fluid, PhysFluid::None) || same_fluid(lava, fluid)
}

fn same_fluid(lava: bool, fluid: PhysFluid) -> bool {
    matches!((lava, fluid), (false, PhysFluid::Water { .. }) | (true, PhysFluid::Lava { .. }))
}

/// `BlockState.blocksMotion`, approximated by "has collision boxes".
/// Vanilla's is `!cobweb && !bamboo_sapling && isSolid()`; what the flow reads
/// (the floor under a drop-off, a wall) is solid either way.
fn blocks_motion(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32) -> bool {
    !world.collision(x, y, z).is_empty()
}

/// `FlowingFluid.isSolidFace` for a horizontal `direction` — the only kind
/// `getFlow` asks, so its `direction == UP` arm never runs. Two
/// approximations: vanilla exempts `IceBlock`, which `BlockPhysics` cannot
/// tell from stone (a falling column beside ice is pulled down here and not
/// in vanilla), and `SupportType.FULL` checks the union of the neighbour's
/// box faces where here one box must span the whole face.
fn solid_face(world: &dyn PhysicsWorld, x: i32, y: i32, z: i32, d: (i32, i32), lava: bool) -> bool {
    if same_fluid(lava, world.block(x, y, z).fluid) {
        return false;
    }
    let axis = if d.0 != 0 { 0 } else { 2 };
    full_face(world.collision(x, y, z), axis, d.0 > 0 || d.1 > 0)
}

/// One box fills the cell's whole `axis` face: flush with it, and spanning the
/// cell on the other two axes.
fn full_face(boxes: &[[f32; 6]], axis: usize, positive: bool) -> bool {
    boxes.iter().any(|b| {
        (0..3).all(|a| {
            if a == axis {
                if positive { b[a + 3] == 1.0 } else { b[a] == 0.0 }
            } else {
                b[a] == 0.0 && b[a + 3] == 1.0
            }
        })
    })
}

/// `Vec3.normalize`: `Vec3.ZERO` below `1.0E-5F`, not a division by zero.
fn normalize_vec(v: [f64; 3]) -> [f64; 3] {
    let d = len_sq(v).sqrt();
    if d < 1.0e-5f32 as f64 { [0.0; 3] } else { [v[0] / d, v[1] / d, v[2] / d] }
}

/// `Vec3.scale`.
fn scale_vec(v: [f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

/// `Vec3.add`.
fn add_vec(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn contains_liquid(b: &Aabb, world: &dyn PhysicsWorld) -> bool {
    for y in b[1].floor() as i32..b[4].ceil() as i32 {
        for z in b[2].floor() as i32..b[5].ceil() as i32 {
            for x in b[0].floor() as i32..b[3].ceil() as i32 {
                if world.block(x, y, z).fluid != PhysFluid::None {
                    return true;
                }
            }
        }
    }
    false
}

// -- collision ---------------------------------------------------------------

/// `[minX, minY, minZ, maxX, maxY, maxZ]`, absolute.
type Aabb = [f64; 6];

fn len_sq(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn offset(b: &Aabb, d: [f64; 3]) -> Aabb {
    [b[0] + d[0], b[1] + d[1], b[2] + d[2], b[3] + d[0], b[4] + d[1], b[5] + d[2]]
}

fn deflate(b: &Aabb, e: f64) -> Aabb {
    [b[0] + e, b[1] + e, b[2] + e, b[3] - e, b[4] - e, b[5] - e]
}

/// `AABB.expandTowards`.
fn expand_towards(b: &Aabb, d: [f64; 3]) -> Aabb {
    let mut o = *b;
    for a in 0..3 {
        if d[a] < 0.0 {
            o[a] += d[a];
        } else if d[a] > 0.0 {
            o[a + 3] += d[a];
        }
    }
    o
}

/// Positive-volume overlap (`AABB.intersects`, and `joinIsNotEmpty(AND)`).
fn intersects(a: &Aabb, b: &Aabb) -> bool {
    a[0] < b[3] && a[3] > b[0] && a[1] < b[4] && a[4] > b[1] && a[2] < b[5] && a[5] > b[2]
}

/// `BlockCollisions` over `query`: every block shape that overlaps it, in
/// `Cursor3D` order (x fastest, then y, then z). The scan covers the box
/// plus a one-block shell; a shell cell on one face counts only for a shape
/// reaching outside its own block (`hasLargeCollisionShape` — a fence or wall
/// below the box), edge and corner cells never (moving pistons aside).
fn for_each_collider(query: &Aabb, world: &dyn PhysicsWorld, mut f: impl FnMut([i32; 3], &[Aabb])) {
    let lo = |v: f64| (v - EPS).floor() as i32 - 1;
    let hi = |v: f64| (v + EPS).floor() as i32 + 1;
    let (x0, y0, z0, x1, y1, z1) = (lo(query[0]), lo(query[1]), lo(query[2]), hi(query[3]), hi(query[4]), hi(query[5]));
    let mut boxes: Vec<Aabb> = Vec::with_capacity(4);
    for z in z0..=z1 {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let kind = (x == x0 || x == x1) as u8 + (y == y0 || y == y1) as u8 + (z == z0 || z == z1) as u8;
                if kind >= 2 {
                    continue;
                }
                let shape = world.collision(x, y, z);
                if shape.is_empty() {
                    continue;
                }
                if kind == 1
                    && !shape.iter().any(|b| b[..3].iter().any(|v| *v < 0.0) || b[3..].iter().any(|v| *v > 1.0))
                {
                    continue;
                }
                boxes.clear();
                boxes.extend(shape.iter().map(|b| {
                    [
                        x as f64 + b[0] as f64,
                        y as f64 + b[1] as f64,
                        z as f64 + b[2] as f64,
                        x as f64 + b[3] as f64,
                        y as f64 + b[4] as f64,
                        z as f64 + b[5] as f64,
                    ]
                }));
                if boxes.iter().any(|b| intersects(b, query)) {
                    f([x, y, z], &boxes);
                }
            }
        }
    }
}

fn collect_colliders(query: &Aabb, world: &dyn PhysicsWorld) -> Vec<Vec<Aabb>> {
    let mut out = Vec::new();
    for_each_collider(query, world, |_, b| out.push(b.to_vec()));
    out
}

fn no_collision(query: &Aabb, world: &dyn PhysicsWorld) -> bool {
    let mut any = false;
    for_each_collider(query, world, |_, _| any = true);
    !any
}

/// `VoxelShape.collide` for one shape (a union of boxes) along `axis`.
fn shape_collide(axis: usize, shape: &[Aabb], moving: &Aabb, mut d: f64) -> f64 {
    if d.abs() < EPS {
        return 0.0;
    }
    let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
    for s in shape {
        let overlaps = |k: usize| s[k] <= moving[k + 3] - EPS && s[k + 3] > moving[k] + EPS;
        if !overlaps(b) || !overlaps(c) {
            continue;
        }
        if d > 0.0 {
            if s[axis] > moving[axis + 3] - EPS {
                d = d.min(s[axis] - moving[axis + 3]);
            }
        } else if s[axis + 3] <= moving[axis] + EPS {
            d = d.max(s[axis + 3] - moving[axis]);
        }
    }
    d
}

/// `Shapes.collide` over a collider list.
fn shapes_collide(axis: usize, moving: &Aabb, colliders: &[Vec<Aabb>], mut d: f64) -> f64 {
    for shape in colliders {
        if d.abs() < EPS {
            return 0.0;
        }
        d = shape_collide(axis, shape, moving, d);
    }
    d
}

/// `Entity.collideWithShapes`: Y first, then the horizontal axis with the
/// smaller movement, then the larger (`Direction.axisStepOrder`). The world
/// border is one more collider, infinite in Y.
fn collide_with_shapes(
    movement: [f64; 3],
    bx: &Aabb,
    colliders: &[Vec<Aabb>],
    border: Option<BorderCollision>,
) -> [f64; 3] {
    let order = if movement[0].abs() < movement[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
    let mut resolved = [0.0; 3];
    for axis in order {
        let m = movement[axis];
        if m == 0.0 {
            continue;
        }
        let moved = offset(bx, resolved);
        let mut r = shapes_collide(axis, &moved, colliders, m);
        if let Some(b) = border {
            let walls = if axis == 0 {
                Some((b.plane_min_x(), b.plane_max_x()))
            } else if axis == 2 {
                Some((b.plane_min_z(), b.plane_max_z()))
            } else {
                None
            };
            r = clip_border(r, moved[axis], moved[axis + 3], walls);
        }
        resolved[axis] = r;
    }
    resolved
}

/// `Entity.collide`, with the step-up retry.
fn collide(state: &PlayerState, ctx: &Ctx, movement: [f64; 3]) -> [f64; 3] {
    let bx = state.aabb();
    // `collectCollidersIgnoringWorldBorder` decides once, with the query box,
    // whether the border joins the colliders.
    let border_for = |q: &Aabb| {
        ctx.border.filter(|b| {
            b.is_inside_close_to_border(state.x, state.z, q[3] - q[0], q[5] - q[2])
        })
    };
    let step = if len_sq(movement) == 0.0 {
        movement
    } else {
        let q = expand_towards(&bx, movement);
        collide_with_shapes(movement, &bx, &collect_colliders(&q, ctx.world), border_for(&q))
    };
    let x_coll = movement[0] != step[0];
    let y_coll = movement[1] != step[1];
    let z_coll = movement[2] != step[2];
    let landing = y_coll && movement[1] < 0.0;
    let max_step = ctx.attrs.step_height as f32;
    if max_step > 0.0 && (landing || state.on_ground) && (x_coll || z_coll) {
        let grounded = if landing { offset(&bx, [0.0, step[1], 0.0]) } else { bx };
        let mut q = expand_towards(&grounded, [movement[0], max_step as f64, movement[2]]);
        if !landing {
            q = expand_towards(&q, [0.0, -1.0e-5f32 as f64, 0.0]);
        }
        let colliders = collect_colliders(&q, ctx.world);
        let border = border_for(&q);
        for h in step_up_candidates(&grounded, &colliders, max_step, step[1] as f32) {
            let s = collide_with_shapes([movement[0], h as f64, movement[2]], &grounded, &colliders, border);
            if s[0] * s[0] + s[2] * s[2] > step[0] * step[0] + step[2] * step[2] {
                let to_ground = bx[1] - grounded[1];
                return [s[0], s[1] - to_ground, s[2]];
            }
        }
    }
    step
}

/// `Entity.collectCandidateStepUpHeights`: every collider y-coordinate at or
/// above the feet and within the step height, as floats, ascending.
fn step_up_candidates(bx: &Aabb, colliders: &[Vec<Aabb>], max_step: f32, skip: f32) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::new();
    for shape in colliders {
        let mut coords: Vec<f64> = shape.iter().flat_map(|b| [b[1], b[4]]).collect();
        coords.sort_by(|a, b| a.partial_cmp(b).unwrap());
        coords.dedup();
        for c in coords {
            let rel = (c - bx[1]) as f32;
            if !(rel < 0.0) && rel != skip {
                if rel > max_step {
                    break;
                }
                if !out.contains(&rel) {
                    out.push(rel);
                }
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// Clip one axis of an already-block-clipped step against the border's two
/// walls on that axis.
///
/// The border's collision shape is the complement of a box, and as a *set* that
/// complement is the union of four half-spaces — `x < lo`, `x > hi`, `z < lo`,
/// `z > hi` — each infinite in the other two axes. So it decomposes into a
/// per-axis clip that needs no cross-axis overlap test, and taking the tighter
/// of the block result and this one is what putting both in vanilla's single
/// collider list does.
///
/// The `gap >= -EPS` guard is the voxel collide's, and it is the reason a
/// player who is *already* past the wall is not frozen: a face behind you does
/// not clip you, so you can walk back in.
fn clip_border(moved: f64, box_min: f64, box_max: f64, walls: Option<(f64, f64)>) -> f64 {
    let Some((lo, hi)) = walls else {
        return moved;
    };
    if moved > 0.0 {
        let gap = hi - box_max;
        if gap >= -EPS && gap < moved {
            return gap.max(0.0);
        }
    } else if moved < 0.0 {
        let gap = lo - box_min;
        if gap <= EPS && gap > moved {
            return gap.min(0.0);
        }
    }
    moved
}

/// A [`PhysicsWorld`] over a [`crate::World`] and the baked per-state tables
/// (`BakedAssets::collide` / `BakedAssets::physics`). A state past the end of
/// `collide` collides as a full cube unless it is air, and one past the end of
/// `blocks` behaves like stone — the flat-test-world fallbacks.
pub struct WorldPhysics<'a> {
    pub world: &'a crate::World,
    pub collide: &'a [Vec<[f32; 6]>],
    pub blocks: &'a [BlockPhysics],
}

const FULL_CUBE: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

impl PhysicsWorld for WorldPhysics<'_> {
    fn collision(&self, x: i32, y: i32, z: i32) -> &[[f32; 6]] {
        let state = self.world.block_state_at(x, y, z);
        match self.collide.get(state as usize) {
            Some(b) => b,
            None if state != 0 => FULL_CUBE,
            None => &[],
        }
    }
    fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
        let state = self.world.block_state_at(x, y, z);
        match self.blocks.get(state as usize) {
            Some(b) => *b,
            None if state == 0 => BlockPhysics::AIR,
            None => BlockPhysics::default(),
        }
    }
    fn has_chunk(&self, x: i32, z: i32) -> bool {
        self.world.is_loaded(x, z)
    }
    fn min_y(&self) -> i32 {
        self.world.shape.min_y
    }
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
        assert!((dist - walked_from_rest(0.1)).abs() < 1e-12, "40-tick walk = {dist}");
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
        let speed = (0.1f32 as f64 * 1.3) as f32;
        assert!((dist - walked_from_rest(speed)).abs() < 1e-12, "40-tick sprint = {dist}");
    }

    /// 40 ticks of ground movement from rest, from the formulas: each tick
    /// adds the accel `speed · (0.21600002F / 0.6F³) · 0.98F`, moves by the
    /// result, then multiplies by `0.6F · 0.91F`.
    fn walked_from_rest(speed: f32) -> f64 {
        let f = 0.6f32;
        let accel = (speed * (0.21600002f32 / (f * f * f))) as f64 * (0.98f32 as f64);
        let drag = (f * 0.91f32) as f64;
        let (mut v, mut d) = (0.0f64, 0.0f64);
        for _ in 0..40 {
            v += accel;
            d += v;
            v *= drag;
        }
        d
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
        // Gravity, then the `0.98F` vertical drag (a float, widened).
        assert_eq!(p.vy, -0.08 * (0.98f32 as f64), "vy={} — entered at 0", p.vy);
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
            tick_with(&mut p, &TickInput::default(), &a, false, None, &air);
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
            tick_with(&mut p, &up, &ab, false, None, &air);
        }
        // I is the f32-computed 0.15f, so the fixed point is 1.5·I exactly.
        let i = crate::abilities::Abilities::default().vertical_flight_impulse(true, false);
        assert!((p.vy - 1.5 * i).abs() < 1e-12, "carried vy={}, want 1.5·I", p.vy);
        let before = p.y;
        fc.before_travel(&mut ab, &mut p, &up, false, false);
        tick_with(&mut p, &up, &ab, false, None, &air);
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
                tick_with(&mut p, &input, &a, false, None, &air);
            }
            p.vz
        };
        let want = |accel: f64| {
            let drag = 0.91f32 as f64;
            accel * (0.98f32 as f64) * drag / (1.0 - drag)
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
                tick_with(&mut p, &input, &a, false, None, &air);
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
            tick_with(&mut p, &down, &ab, false, None, &world);
        }
        assert!(p.y >= -1e-9, "flying down stops at the floor, y={}", p.y);

        // The same descent with no-clip sinks straight through it.
        let mut q = PlayerState::at(0.5, 1.0, 0.5);
        let mut fc2 = crate::abilities::FlightControl::default();
        let mut ab2 = flying();
        for _ in 0..60 {
            fc2.before_travel(&mut ab2, &mut q, &down, false, false);
            tick_with(&mut q, &down, &ab2, true, None, &world);
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
            tick_with(&mut b, &input, &Abilities::default(), false, None, &world);
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
        // Holding jump re-jumps on every landing, so assert it *landed* on
        // the ledge at some tick rather than where it is at the end.
        let mut landed = false;
        for _ in 0..80 {
            tick(&mut p, &input, &world);
            landed |= p.on_ground && p.z > 3.0 && p.y == 1.0;
        }
        assert!(p.z > 4.0, "made it onto the ledge, z={}", p.z);
        assert!(landed, "stood on the ledge top at some tick");

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

    // ── The world border as a collider (M80) ──────────────────────────────

    /// A border centred on the origin, `size` across, already ticked into a
    /// static extent.
    fn border(size: f64) -> crate::border::BorderCollision {
        let mut b = crate::border::WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(size);
        b.collision()
    }

    #[test]
    fn the_border_stops_a_walking_player_at_the_wall() {
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let wall = border(20.0); // ±10
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut p = PlayerState::at(0.0, 0.0, 5.0);
        for _ in 0..200 {
            tick_with(
                &mut p,
                &input,
                &Abilities::default(),
                false,
                Some(wall),
                &world,
            );
        }
        assert!(
            p.z <= 10.0 - PLAYER_HALF_WIDTH + 1e-6,
            "the box's far face rests on the wall, z={}",
            p.z
        );
        assert!(p.z > 9.0, "and got there, z={}", p.z);
        assert!(p.horizontal_collision, "the border reports as a collision");

        // The mutation partner: the same 200 ticks with no border passed. The
        // walk is otherwise identical, so any stop above is the border and not
        // the floor, the drag, or a tick-count artefact.
        let mut q = PlayerState::at(0.0, 0.0, 5.0);
        for _ in 0..200 {
            tick(&mut q, &input, &world);
        }
        assert!(q.z > 20.0, "unbordered, the same walk goes far past, z={}", q.z);
        assert!(!q.horizontal_collision);
    }

    #[test]
    fn a_player_left_outside_the_border_can_walk_back_in() {
        // `isInsideCloseToBorder` withholds the collider once you are further
        // than your own width outside. Without that gate the shape — an
        // infinite complement — would seal an outside player in place.
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let wall = border(20.0);
        let input = TickInput {
            forward: -1.0, // south → north, back toward the border
            ..Default::default()
        };
        let mut p = PlayerState::at(0.0, 0.0, 14.0);
        for _ in 0..200 {
            tick_with(
                &mut p,
                &input,
                &Abilities::default(),
                false,
                Some(wall),
                &world,
            );
        }
        assert!(p.z < 12.0, "walked back toward the wall, z={}", p.z);
    }

    #[test]
    fn the_border_does_not_clip_vertical_movement() {
        // The collision shape is infinite in Y, so standing at the wall must
        // not interfere with falling onto the floor.
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let wall = border(20.0);
        let mut p = PlayerState::at(0.0, 6.0, 9.9);
        for _ in 0..60 {
            tick_with(
                &mut p,
                &TickInput::default(),
                &Abilities::default(),
                false,
                Some(wall),
                &world,
            );
        }
        assert!(p.on_ground, "landed");
        assert!(p.y.abs() < 1e-6, "on the floor, y={}", p.y);
    }

    #[test]
    fn a_spectator_passes_through_the_border() {
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let wall = border(20.0);
        let mut a = Abilities::default();
        a.flying = true;
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut p = PlayerState::at(0.0, 4.0, 5.0);
        for _ in 0..200 {
            tick_with(&mut p, &input, &a, true, Some(wall), &world);
        }
        assert!(p.z > 20.0, "no-clip ignores the wall, z={}", p.z);
    }

    #[test]
    fn the_collider_plane_is_the_floored_wall_not_the_exact_one() {
        // A fractional border collides on whole-block boundaries — the box in
        // `getCollisionShape` is floored and ceiled. The visible wall is at
        // 10.25; the one you bump into is at 11.
        let world = |_x: i32, y: i32, _z: i32| cube(y < 0);
        let mut b = crate::border::WorldBorder::default();
        b.set_center(0.0, 0.0);
        b.set_size(20.5); // ±10.25
        let wall = b.collision();
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut p = PlayerState::at(0.0, 0.0, 5.0);
        for _ in 0..200 {
            tick_with(
                &mut p,
                &input,
                &Abilities::default(),
                false,
                Some(wall),
                &world,
            );
        }
        assert!(
            p.z > 10.25 - PLAYER_HALF_WIDTH,
            "walked past the *visible* wall, z={}",
            p.z
        );
        assert!(
            p.z <= 11.0 - PLAYER_HALF_WIDTH + 1e-6,
            "and stopped at the *floored* one, z={}",
            p.z
        );
    }

    // ---- vanilla-derived expectations for the full tick ------------------

    struct TestWorld<S: Fn(i32, i32, i32) -> &'static [[f32; 6]], B: Fn(i32, i32, i32) -> BlockPhysics> {
        shape: S,
        block: B,
        loaded: bool,
    }

    impl<S: Fn(i32, i32, i32) -> &'static [[f32; 6]], B: Fn(i32, i32, i32) -> BlockPhysics> PhysicsWorld
        for TestWorld<S, B>
    {
        fn collision(&self, x: i32, y: i32, z: i32) -> &[[f32; 6]] {
            (self.shape)(x, y, z)
        }
        fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
            (self.block)(x, y, z)
        }
        fn has_chunk(&self, _x: i32, _z: i32) -> bool {
            self.loaded
        }
        fn min_y(&self) -> i32 {
            -64
        }
    }

    fn stone() -> BlockPhysics {
        BlockPhysics::default()
    }

    fn step_env(p: &mut PlayerState, input: &TickInput, w: &dyn PhysicsWorld) {
        tick_env(p, input, &Abilities::default(), false, None, &MoveAttributes::default(), w);
    }

    /// Ground terminal speed from the formulas alone: the accel
    /// `speed * (0.21600002F / f³)` (a float), the 0.98F input, and the
    /// `f * 0.91F` drag applied to the accelerated velocity.
    fn ground_terminal(speed: f32, friction: f32) -> f64 {
        let accel = if friction as f64 > 0.6 {
            speed * (0.21600002f32 / (friction * friction * friction))
        } else {
            speed
        } as f64
            * (0.98f32 as f64);
        let drag = (friction * 0.91f32) as f64;
        accel * drag / (1.0 - drag)
    }

    #[test]
    fn walk_and_sprint_terminals_are_exact() {
        let w = TestWorld { shape: |_, y, _| cube(y < 0), block: |_, _, _| stone(), loaded: true };
        for (sprint, speed) in [(false, 0.1f32), (true, (0.1f32 as f64 * 1.3) as f32)] {
            let mut p = PlayerState::at(0.5, 0.0, 0.5);
            let input = TickInput { forward: 1.0, sprint, ..Default::default() };
            for _ in 0..400 {
                step_env(&mut p, &input, &w);
            }
            let want = ground_terminal(speed, 0.6);
            assert!((p.vz - want).abs() < 1e-12, "sprint={sprint}: vz={} want {want}", p.vz);
        }
    }

    /// Ice (0.98) and blue ice (0.989) come from the block *below the
    /// feet*, through the `0.21600002F / f³` accel.
    #[test]
    fn ice_friction_terminal_is_exact() {
        for f in [0.98f32, 0.989] {
            let w = TestWorld {
                shape: |_, y, _| cube(y < 0),
                block: move |_, y, _| if y < 0 { BlockPhysics { friction: f, ..stone() } } else { BlockPhysics::AIR },
                loaded: true,
            };
            let mut p = PlayerState::at(0.5, 0.0, 0.5);
            let input = TickInput { forward: 1.0, ..Default::default() };
            for _ in 0..3000 {
                step_env(&mut p, &input, &w);
            }
            let want = ground_terminal(0.1, f);
            assert!((p.vz - want).abs() < 1e-9, "friction {f}: vz={} want {want}", p.vz);
        }
    }

    /// Soul sand's 0.4 speed factor multiplies the horizontal velocity after
    /// every move (read from the block below: the feet block is air).
    #[test]
    fn soul_sand_speed_factor_scales_velocity() {
        let w = TestWorld {
            shape: |_, y, _| cube(y < 0),
            block: |_, y, _| if y < 0 { BlockPhysics { speed_factor: 0.4, ..stone() } } else { BlockPhysics::AIR },
            loaded: true,
        };
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        let input = TickInput { forward: 1.0, ..Default::default() };
        for _ in 0..400 {
            step_env(&mut p, &input, &w);
        }
        let accel = 0.1f32 as f64 * (0.98f32 as f64);
        let drag = (0.6f32 * 0.91f32) as f64;
        // v = ((v + a) * 0.4) * drag at the fixed point.
        let f = 0.4f32 as f64;
        let want = accel * f * drag / (1.0 - f * drag);
        assert!((p.vz - want).abs() < 1e-12, "vz={} want {want}", p.vz);
    }

    /// The jump apex from `0.42F` and the per-tick `(vy − 0.08) × 0.98F`.
    #[test]
    fn jump_apex_is_exact() {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        tick(&mut p, &TickInput { jump: true, ..Default::default() }, &floor);
        let mut apex = p.y;
        for _ in 0..20 {
            tick(&mut p, &TickInput::default(), &floor);
            apex = apex.max(p.y);
        }
        let (mut y, mut vy, mut best) = (0.0f64, 0.42f32 as f64, 0.0f64);
        for _ in 0..21 {
            y += vy;
            best = best.max(y);
            vy = (vy - 0.08) * (0.98f32 as f64);
        }
        assert!((apex - best).abs() < 1e-12, "apex {apex} want {best}");
    }

    /// Item 2 of the collision port: a fence post collides 1.5 tall, and the
    /// block whose shape reaches up into the box must be scanned even when
    /// the box starts above that block (`BlockCollisions`' shell).
    const POST: &[[f32; 6]] = &[[0.375, 0.0, 0.375, 0.625, 1.5, 0.625]];
    const WALL15: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.5, 1.0]];

    #[test]
    fn jumping_into_a_fence_does_not_pass_it() {
        let world = |_x: i32, y: i32, z: i32| {
            if y < 0 {
                FULL
            } else if z == 2 && y == 0 {
                POST
            } else {
                EMPTY
            }
        };
        let mut p = PlayerState::at(2.5, 0.0, 0.5);
        let input = TickInput { forward: 1.0, jump: true, sprint: true, ..Default::default() };
        for _ in 0..100 {
            tick(&mut p, &input, &world);
            assert!(p.z < 2.375 - PLAYER_HALF_WIDTH + 1e-6, "passed the post: z={} y={}", p.z, p.y);
        }
    }

    #[test]
    fn standing_on_a_tall_collider_does_not_fall_through() {
        let world = |x: i32, y: i32, z: i32| if (x, y, z) == (0, 0, 0) { WALL15 } else { EMPTY };
        let mut p = PlayerState::at(0.5, 1.5, 0.5);
        for _ in 0..40 {
            tick(&mut p, &TickInput::default(), &world);
        }
        assert!(p.on_ground);
        assert_eq!(p.y, 1.5, "rests on the 1.5-tall top");
    }

    /// `Direction.axisStepOrder`: with |dx| < |dz`| Z is resolved before X.
    #[test]
    fn the_smaller_horizontal_axis_is_resolved_last() {
        let block: Vec<Aabb> = vec![[1.0, 0.0, 1.0, 2.0, 1.0, 2.0]];
        let bx: Aabb = [0.35, 0.0, 0.2, 0.95, 1.8, 0.8];
        let r = collide_with_shapes([0.1, 0.0, 0.5], &bx, &[block.clone()], None);
        assert!((r[0] - 0.05).abs() < 1e-12 && (r[2] - 0.5).abs() < 1e-12, "{r:?}");
        // The mirrored case resolves X first.
        let r = collide_with_shapes([0.5, 0.0, 0.1], &[0.2, 0.0, 0.35, 0.8, 1.8, 0.95], &[block], None);
        assert!((r[0] - 0.5).abs() < 1e-12 && (r[2] - 0.05).abs() < 1e-12, "{r:?}");
    }

    /// No step-up in the air: only on the ground or when landing this tick.
    #[test]
    fn no_step_up_while_airborne() {
        const SLAB: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let world = |_x: i32, y: i32, z: i32| if z >= 2 && y == 5 { SLAB } else { EMPTY };
        let mut p = PlayerState::at(0.5, 5.2, 1.6);
        p.vz = 0.3;
        p.vy = 0.0;
        tick(&mut p, &TickInput::default(), &world);
        assert!(p.horizontal_collision);
        assert_eq!(p.y, 5.2, "did not step up onto the slab");
    }

    /// Climbing: into a wall on a ladder, `vy` is set to 0.2 after the move,
    /// then gravity and the 0.98F drag apply.
    #[test]
    fn a_ladder_climbs_at_the_vanilla_rate() {
        let ladder = BlockPhysics { flags: flags::CLIMBABLE, ..BlockPhysics::AIR };
        let w = TestWorld {
            shape: |_, y, z| cube(y < 0 || z >= 1),
            block: move |_, y, z| if z == 0 && y >= 0 { ladder } else { stone() },
            loaded: true,
        };
        let mut p = PlayerState::at(0.5, 0.0, 0.7);
        let input = TickInput { forward: 1.0, ..Default::default() };
        for _ in 0..5 {
            step_env(&mut p, &input, &w);
        }
        let y0 = p.y;
        step_env(&mut p, &input, &w);
        let per_tick = (0.2 - 0.08) * (0.98f32 as f64);
        assert!((p.y - y0 - per_tick).abs() < 1e-12, "climbed {}", p.y - y0);
        // Sneaking with no input holds position.
        let hold = TickInput { sneak: true, ..Default::default() };
        for _ in 0..10 {
            step_env(&mut p, &hold, &w);
        }
        let y1 = p.y;
        for _ in 0..10 {
            step_env(&mut p, &hold, &w);
        }
        assert_eq!(p.y, y1, "sneak holds on the ladder");
    }

    /// A cobweb stores `(0.25, 0.05F, 0.25)`; the next move is scaled by it
    /// and the velocity is zeroed.
    #[test]
    fn a_cobweb_slows_the_next_move() {
        let web = BlockPhysics { stuck: Stuck::Web, ..BlockPhysics::AIR };
        let w = TestWorld { shape: |_, _, _| EMPTY, block: move |_, _, _| web, loaded: true };
        let mut p = PlayerState::at(0.5, 10.0, 0.5);
        p.vy = -1.0;
        step_env(&mut p, &TickInput::default(), &w); // moves freely, then gets stuck
        assert_eq!(p.stuck_multiplier, [0.25, 0.05f32 as f64, 0.25]);
        let (y0, vy) = (p.y, p.vy);
        step_env(&mut p, &TickInput::default(), &w);
        assert!((p.y - y0 - vy * (0.05f32 as f64)).abs() < 1e-12);
    }

    /// Sneaking stops at the edge: the box may overhang but never so far that
    /// it could fall more than a step.
    #[test]
    fn sneaking_does_not_walk_off_an_edge() {
        let world = |x: i32, y: i32, _z: i32| cube(y < 0 && x <= 0);
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        settle(&mut p);
        let input = TickInput { strafe: 1.0, sneak: true, ..Default::default() }; // yaw 0: left = +x
        for _ in 0..200 {
            tick(&mut p, &input, &world);
        }
        assert!(p.on_ground, "still on the block");
        assert!(p.x > 1.0 && p.x < 1.0 + PLAYER_HALF_WIDTH, "overhangs but stays: x={}", p.x);
    }

    /// Deep water, holding jump: `+0.04F` per tick, the `0.8F` drag and the
    /// `gravity / 16` fall adjustment.
    #[test]
    fn swimming_up_in_deep_water() {
        let water = BlockPhysics { fluid: PhysFluid::Water { own_height: 8.0 / 9.0 }, ..BlockPhysics::AIR };
        let w = TestWorld { shape: |_, _, _| EMPTY, block: move |_, y, _| if y < 20 { water } else { BlockPhysics::AIR }, loaded: true };
        let mut p = PlayerState::at(0.5, 5.0, 0.5);
        let jump = TickInput { jump: true, ..Default::default() };
        let mut vy = 0.0f64;
        for _ in 0..6 {
            step_env(&mut p, &jump, &w);
            let v = if vy.abs() < 0.003 { 0.0 } else { vy };
            let falling = v <= 0.0;
            // `getFluidFallingAdjustedMovement` with gravity/16 = 0.005: its
            // -0.003 arm needs |n − 0.005| both ≥ and < 0.003, so it never
            // fires here.
            let _ = falling;
            vy = (v + 0.04f32 as f64) * (0.8f32 as f64) - 0.08 / 16.0;
            assert!((p.vy - vy).abs() < 1e-12, "vy {} want {vy}", p.vy);
        }
        assert!(p.in_water() && p.vy > 0.0);
    }

    /// An unloaded chunk below: vanilla falls at a fixed `-0.1` (then the
    /// 0.98F drag) rather than accelerating.
    #[test]
    fn an_unloaded_chunk_falls_at_minus_point_one() {
        let w = TestWorld { shape: |_, _, _| EMPTY, block: |_, _, _| BlockPhysics::AIR, loaded: false };
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        for _ in 0..5 {
            step_env(&mut p, &TickInput::default(), &w);
        }
        assert_eq!(p.vy, -0.1 * (0.98f32 as f64));
    }

    /// A slime block bounces a non-sneaking landing: restitution 1.0.
    #[test]
    fn slime_bounces_and_sneaking_suppresses_it() {
        let slime = BlockPhysics { bounce: 1.0, friction: 0.8, ..stone() };
        let w = TestWorld { shape: |_, y, _| cube(y < 0), block: move |_, y, _| if y < 0 { slime } else { BlockPhysics::AIR }, loaded: true };
        let run = |sneak: bool| {
            let mut p = PlayerState::at(0.5, 3.0, 0.5);
            let input = TickInput { sneak, ..Default::default() };
            let mut up = false;
            for _ in 0..30 {
                step_env(&mut p, &input, &w);
                up |= p.vy > 0.1;
            }
            up
        };
        assert!(run(false), "bounced");
        assert!(!run(true), "sneaking lands flat");
    }

    // ── bubble columns and honey wall sliding ────────────────────────────

    /// The physics the bake fills in for a water source around a column
    /// (`FluidState.getOwnHeight()` of a source = 8/9).
    fn source_water() -> BlockPhysics {
        BlockPhysics { fluid: PhysFluid::Water { own_height: 8.0 / 9.0 }, ..BlockPhysics::AIR }
    }

    /// One `BubbleColumnBlock` cell at (0, 0, 0) inside source water over a
    /// stone floor. The cell over the column is water, so vanilla's
    /// `nothingAbove` is false and the *inside* arm applies.
    fn inside_column(drag_down: bool) -> impl Fn(i32, i32, i32) -> BlockPhysics {
        let flag = if drag_down { flags::BUBBLE_COLUMN_DOWN } else { flags::BUBBLE_COLUMN_UP };
        let column = BlockPhysics { flags: flag, ..source_water() };
        move |x, y, z| match (x, y, z) {
            (0, 0, 0) => column,
            (0, 1..=3, 0) => source_water(),
            _ => BlockPhysics::AIR,
        }
    }

    /// `BubbleColumnBlock.entityInside`'s inside arm on a push-up column: the
    /// player standing in the column ends the tick at
    /// `Entity.handleOnInsideBubbleColumn`'s `min(0.7, vy + 0.06)`.
    #[test]
    fn bubble_column_up_pushes_upward_inside() {
        let w = TestWorld { shape: |_, y, _| cube(y < 0), block: inside_column(false), loaded: true };
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        // From rest the water arm ends at `vy − gravity/16` = −0.005
        // (`getFluidFallingAdjustedMovement`), then `min(0.7, vy + 0.06)`.
        let want = (-0.08 / 16.0 + 0.06f64).min(0.7);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
        assert!(p.vy > 0.0, "pushes upward, vy={}", p.vy);
    }

    /// The inside arm on a drag-down column: `max(-0.3, vy − 0.03)`.
    #[test]
    fn bubble_column_down_pulls_down_inside() {
        let w = TestWorld { shape: |_, y, _| cube(y < 0), block: inside_column(true), loaded: true };
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        let rest = -0.08 / 16.0;
        let want = (rest - 0.03f64).max(-0.3);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
        assert!(p.vy < rest, "pulls down past the unassisted tick, vy={}", p.vy);
    }

    /// The *above* arm: air over the column makes vanilla's `nothingAbove`
    /// true, and `Entity.handleOnAboveBubbleColumn` gives
    /// `min(1.8, vy + 0.1)`.
    #[test]
    fn bubble_column_above_surface() {
        let column = BlockPhysics { flags: flags::BUBBLE_COLUMN_UP, ..source_water() };
        let w = TestWorld {
            shape: |_, y, _| cube(y < 0),
            block: move |x, y, z| match (x, y, z) {
                (0, 0, 0) => source_water(),
                (0, 1, 0) => column,
                _ => BlockPhysics::AIR,
            },
            loaded: true,
        };
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        // The same −0.005 at the effect, then `min(1.8, vy + 0.1)`.
        let want = (-0.08 / 16.0 + 0.1f64).min(1.8);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
    }

    /// A single `HoneyBlock` at (0, 0, 0) with the baked 14/16 shape
    /// (`HoneyBlock.SHAPE` = `Block.column(14.0, 0.0, 15.0)`).
    fn honey_shape(x: i32, y: i32, z: i32) -> &'static [[f32; 6]] {
        const SHAPE: &[[f32; 6]] = &[[0.0625, 0.0, 0.0625, 0.9375, 0.9375, 0.9375]];
        if (x, y, z) == (0, 0, 0) {
            SHAPE
        } else {
            EMPTY
        }
    }

    /// What `BlockPhysics::resolve("minecraft:honey_block", …)` returns,
    /// placed at (0, 0, 0).
    fn honey_block(x: i32, y: i32, z: i32) -> BlockPhysics {
        if (x, y, z) == (0, 0, 0) {
            BlockPhysics { flags: flags::HONEY, speed_factor: 0.4, jump_factor: 0.5, ..BlockPhysics::AIR }
        } else {
            BlockPhysics::AIR
        }
    }

    /// `HoneyBlock.doSlideMovement` caps the fall at `getNewDeltaY(-0.05)`
    /// for a player sliding down the block's side wall.
    #[test]
    fn honey_wall_slide_caps_fall_speed() {
        let w = TestWorld { shape: honey_shape, block: honey_block, loaded: true };
        // Falling alongside the wall: the box overlaps the honey cell but not
        // its 14/16 shape, so nothing collides.
        let mut p = PlayerState::at(-0.25, 0.5, 0.5);
        p.vy = -0.5;
        step_env(&mut p, &TickInput::default(), &w);
        // `doSlideMovement` sets `getNewDeltaY(-0.05)` = (−0.05 − 0.08)·0.98F
        // whatever came in, and resets the fall distance (0.5 from the fall).
        let want = (-0.05 - 0.08) * (0.98f32 as f64);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
        assert_eq!(p.fall_distance, 0.0);
    }

    /// Standing on top of the honey — its shape tops out at 15/16 — is not a
    /// slide: `isSlidingDown` refuses a player over the block's top.
    #[test]
    fn honey_top_does_not_slide() {
        let w = TestWorld { shape: honey_shape, block: honey_block, loaded: true };
        let mut p = PlayerState::at(0.5, 0.9375, 0.5);
        for _ in 0..5 {
            step_env(&mut p, &TickInput::default(), &w);
        }
        assert_eq!(p.y, 0.9375, "did not slide down the wall");
        // The plain standing tick: (vy − 0.08)·0.98F from rest.
        let want = (0.0 - 0.08) * (0.98f32 as f64);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
    }

    /// The slide's y-guard, the sliver where it alone decides: level with the
    /// honey's top but beside its side, where `isSlidingDown`'s overlap test
    /// passes and only `getY() > pos.getY() + 0.9375 - 1.0E-7` rules it out.
    #[test]
    fn honey_top_edge_does_not_slide() {
        let w = TestWorld { shape: honey_shape, block: honey_block, loaded: true };
        // One tick's fall of 0.5 lands the feet at 0.95: above the shape's
        // 0.9375 top, still in the cell the box touches.
        let mut p = PlayerState::at(-0.25, 1.45, 0.5);
        p.vy = -0.5;
        step_env(&mut p, &TickInput::default(), &w);
        // The plain fall: (vy − 0.08)·0.98F, not throttled to −0.13.
        let want = (-0.5 - 0.08) * (0.98f32 as f64);
        assert!((p.vy - want).abs() < 1e-12, "vy={}", p.vy);
        assert_eq!(p.fall_distance, 0.5, "fall distance untouched");
    }

    // ── fluid currents (`FlowingFluid.getFlow`) ──────────────────────────

    fn world(
        shape: impl Fn(i32, i32, i32) -> &'static [[f32; 6]],
        block: impl Fn(i32, i32, i32) -> BlockPhysics,
    ) -> impl PhysicsWorld {
        TestWorld { shape, block, loaded: true }
    }

    /// The bake's water cell for one `level` (`fluid_own_height`), not falling.
    fn water(own_height: f32) -> BlockPhysics {
        BlockPhysics { fluid: PhysFluid::Water { own_height }, ..BlockPhysics::AIR }
    }

    fn lava(own_height: f32) -> BlockPhysics {
        BlockPhysics { fluid: PhysFluid::Lava { own_height }, ..BlockPhysics::AIR }
    }

    /// One cell at (0, 0, 0) with a neighbour at (1, 0, 0), the rest air. A
    /// player at (0.5, 0, 0.5) samples only (0, 0, 0): the interaction box
    /// spans the x = 0 and z = 0 column, y = 0..1.
    fn draining(here: BlockPhysics, there: BlockPhysics) -> impl Fn(i32, i32, i32) -> BlockPhysics {
        move |x, y, z| match (x, y, z) {
            (0, 0, 0) => here,
            (1, 0, 0) => there,
            _ => BlockPhysics::AIR,
        }
    }

    /// Standing in water whose +X neighbour is lower: the flow pushes the
    /// player downstream.
    #[test]
    fn flowing_water_pushes_downstream() {
        let w = world(|_, _, _| EMPTY, draining(water(8.0 / 9.0), water(1.0 / 9.0)));
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        // `getFlow` at (0, 0, 0): the east neighbour's own height 1/9 is below
        // ours 8/9, so `distance = 8/9 − 1/9 = 7/9` and the flow is (7/9, 0, 0)
        // → the unit vector (1, 0, 0). The other three neighbours are air with
        // nothing below (0 each) and the cell is not falling. One cell is
        // sampled, its tracker height 8/9 ≥ 0.4 so the flow is unscaled, and a
        // player gets `sum / count · 0.014` = +0.014 in x (over the 0.0045
        // nudge). `travelInWater` then multiplies by 0.8F.
        let want = 0.014 * (0.8f32 as f64);
        assert!((p.vx - want).abs() < 1e-12, "vx={} want {want}", p.vx);
        assert_eq!(p.vz, 0.0);
        assert!(p.vx > 0.0, "pushed downstream, vx={}", p.vx);
    }

    /// A pool of sources: every neighbour is the same water at the same
    /// height, so `getFlow` is zero and the current moves nothing.
    #[test]
    fn still_water_does_not_push() {
        let w = world(|_, _, _| EMPTY, |_, y, _| if y < 2 { water(8.0 / 9.0) } else { BlockPhysics::AIR });
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        // Every sampled cell sees four neighbours at its own height:
        // `distance = 8/9 − 8/9 = 0` all round, and the state is a source.
        // The accumulated flow is zero, so `applyCurrentTo` adds nothing and
        // the tick is the plain water one: 0 horizontal and the
        // `getFluidFallingAdjustedMovement` `−gravity/16` from rest.
        assert_eq!((p.vx, p.vz), (0.0, 0.0), "still water does not push sideways");
        assert!((p.vy + 0.08 / 16.0).abs() < 1e-12, "vy={}", p.vy);
    }

    /// A falling column beside a solid face: `getFlow`'s falling branch adds
    /// the downward component.
    #[test]
    fn falling_water_pulls_down() {
        let falling = BlockPhysics { fluid_falling: true, ..water(8.0 / 9.0) };
        let w = world(|x, y, z| cube((x, y, z) == (1, 0, 0)), draining(falling, stone()));
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        step_env(&mut p, &TickInput::default(), &w);
        // `getFlow` at (0, 0, 0): the horizontal gradient is zero — the east
        // neighbour is stone, which blocks motion so its cell is never read
        // through, and the other three are air with nothing below. The state is
        // falling though, and the east face of the stone is full, so
        // `flow = (0, 0, 0).normalize() + (0, −6, 0)` = (0, −6, 0) → (0, −1, 0).
        // One cell: `sum / count · 0.014` = (0, −0.014, 0) — over the 0.0045
        // nudge — and `travelInWater` drags by 0.8F and adds `−gravity/16`.
        let want = -0.014 * (0.8f32 as f64) - 0.08 / 16.0;
        assert!((p.vy - want).abs() < 1e-12, "vy={} want {want}", p.vy);
        assert!(p.vy < -0.08 / 16.0, "pulled below the plain tick, vy={}", p.vy);
        assert_eq!((p.vx, p.vz), (0.0, 0.0));
    }

    /// Lava pushes less than water — and its rate is so low that the nudge,
    /// not the rate, is what the player feels.
    #[test]
    fn lava_pushes_slower_than_water() {
        let vx_after_one_tick = |here: BlockPhysics, there: BlockPhysics| {
            let w = world(|_, _, _| EMPTY, draining(here, there));
            let mut p = PlayerState::at(0.5, 0.0, 0.5);
            step_env(&mut p, &TickInput::default(), &w);
            p.vx
        };
        // Water, as `flowing_water_pushes_downstream` derives: 0.014 · 0.8F.
        // The mid-move `checkFallDamage` refresh does not add a second
        // application here: it skips the update while `isInWater()`.
        let water_vx = vx_after_one_tick(water(8.0 / 9.0), water(1.0 / 9.0));
        assert!((water_vx - 0.014 * (0.8f32 as f64)).abs() < 1e-12, "water vx={water_vx}");
        // Lava: the same unit flow, but `updateFluidInteraction` scales it by
        // `0.0023333333333333335` (overworld; `FAST_LAVA` is 0.007). The player
        // is at rest and 0.0023… < 0.0045, so `applyCurrentTo`'s nudge replaces
        // that impulse with `0.0045000000000000005` in the flow's direction.
        // `Entity.move` ends in `checkFallDamage`, which refreshes the fluids
        // because the player is in lava and not in *water* — so the current is
        // applied again mid-tick, now without the nudge (0.0045 > 0.003), and
        // the raw 0.0023… is added. `travelInLava` scales the sum by 0.5.
        let lava_vx = vx_after_one_tick(lava(8.0 / 9.0), lava(1.0 / 9.0));
        assert!((lava_vx - (0.0045000000000000005 + 0.0023333333333333335) * 0.5).abs() < 1e-12, "lava vx={lava_vx}");
        assert!(lava_vx < water_vx, "lava {lava_vx} vs water {water_vx}");
    }
}
