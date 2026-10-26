//! The local player's footsteps: `Entity.applyMovementEmissionAndPlaySound`,
//! `Entity.vibrationAndSoundEffectsFromBlock`, `Player.playStepSound` and the
//! amethyst chime, run after the tick's move.
//!
//! Vanilla plays these through `Player.playSound`, i.e.
//! `level.playSound(player, ...)`: the client plays them for its own player
//! and the server sends them to everyone else, so no packet ever brings the
//! player's own steps back. The game events (`GameEvent.STEP` / `SWIM`) are the
//! server's and are not modelled.

use crate::physics::{self, PhysicsWorld, PlayerState, Pose};
use rewo_data::block_physics::flags;
use rewo_data::sound_types::{tags, StateSound};

/// One sound to play at the player's position, `SoundSource.PLAYERS`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepSound {
    pub name: &'static str,
    pub volume: f32,
    pub pitch: f32,
}

/// What the step logic reads besides the physics state, sampled after the
/// tick's move.
#[derive(Clone, Copy, Debug)]
pub struct StepInput {
    /// `Entity.move`'s collided movement this tick (`movement`, what
    /// `setPos` added).
    pub moved: [f64; 3],
    /// `Abilities.flying`.
    pub flying: bool,
    /// `isShiftKeyDown()` (the sneak key), which `isDiscrete` reads.
    pub shift_down: bool,
    /// `isPassenger()`: a rider emits nothing.
    pub passenger: bool,
    /// The `DATA_SILENT` flag.
    pub silent: bool,
    /// `Entity.tickCount`.
    pub tick_count: i64,
}

/// `Entity`'s step state: `moveDist`, `nextStep` and the crystal chime's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepTracker {
    pub move_dist: f32,
    pub fly_dist: f32,
    pub next_step: f32,
    pub crystal_intensity: f32,
    pub last_crystal_tick: i64,
}

impl Default for StepTracker {
    fn default() -> Self {
        Self {
            move_dist: 0.0,
            fly_dist: 0.0,
            next_step: 1.0,
            crystal_intensity: 0.0,
            last_crystal_tick: 0,
        }
    }
}

impl StepTracker {
    /// Run one tick's emission. `sounds(x, y, z)` is the block's
    /// [`StateSound`]; `random` is the entity's `nextFloat()`.
    pub fn tick(
        &mut self,
        state: &PlayerState,
        input: &StepInput,
        world: &dyn PhysicsWorld,
        sounds: &dyn Fn(i32, i32, i32) -> StateSound,
        random: &mut dyn FnMut() -> f32,
    ) -> Vec<StepSound> {
        let mut out = Vec::new();
        // `Player.getMovementEmission`: NONE while flying or sneaking on the
        // ground; `Entity.move` also skips a passenger.
        let emits = !(input.flying || state.on_ground && input.shift_down);
        if !emits || input.passenger {
            return out;
        }
        let m = input.moved;
        let moved = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt() * 0.6f32 as f64;
        let horizontal = (m[0] * m[0] + m[2] * m[2]).sqrt() * 0.6f32 as f64;
        let effect_pos = physics::on_pos(state, world, 0.2);
        let supporting_pos = physics::on_pos(state, world, 1.0e-5);
        let supporting = world.block(supporting_pos[0], supporting_pos[1], supporting_pos[2]);
        let climbing = climbable(supporting.flags);
        self.move_dist += if climbing {
            moved as f32
        } else {
            horizontal as f32
        };
        self.fly_dist += moved as f32;
        if !(self.move_dist > self.next_step) || supporting.has(flags::AIR) {
            // `processFlappingMovement` is a no-op for a player.
            return out;
        }
        let only_effect = supporting_pos == effect_pos;
        let mut produced = self.effects_from_block(
            effect_pos,
            state,
            input,
            world,
            sounds,
            random,
            !input.silent,
            &mut out,
        );
        if !only_effect {
            // The supporting block only vibrates (`shouldSound` false), but its
            // test still counts as a step.
            produced |= self.effects_from_block(
                supporting_pos,
                state,
                input,
                world,
                sounds,
                random,
                false,
                &mut out,
            );
        }
        if produced {
            self.next_step = self.move_dist as i32 as f32 + 1.0;
        } else if state.in_water() {
            self.next_step = self.move_dist as i32 as f32 + 1.0;
            if !input.silent {
                out.push(swim_sound(state, random));
            }
        }
        out
    }

    /// `vibrationAndSoundEffectsFromBlock`: true when the block counts as a
    /// step (the player is on it, climbing it or sneaking level).
    #[allow(clippy::too_many_arguments)]
    fn effects_from_block(
        &mut self,
        pos: [i32; 3],
        state: &PlayerState,
        input: &StepInput,
        world: &dyn PhysicsWorld,
        sounds: &dyn Fn(i32, i32, i32) -> StateSound,
        random: &mut dyn FnMut() -> f32,
        should_sound: bool,
        out: &mut Vec<StepSound>,
    ) -> bool {
        let block = world.block(pos[0], pos[1], pos[2]);
        if block.has(flags::AIR) {
            return false;
        }
        let crouching = state.pose == Pose::Crouching;
        let steps =
            (state.on_ground || climbable(block.flags) || crouching && input.moved[1] == 0.0)
                && !state.swimming;
        if !steps {
            return false;
        }
        if should_sound {
            let on = sounds(pos[0], pos[1], pos[2]);
            player_step_sound(pos, on, state, sounds, random, out);
            // `shouldPlayAmethystStepSound` / `playAmethystStepSound`.
            if on.has(tags::CRYSTAL) && input.tick_count >= self.last_crystal_tick + 20 {
                let since = (input.tick_count - self.last_crystal_tick) as f64;
                self.crystal_intensity *= 0.997f64.powf(since) as f32;
                self.crystal_intensity = (self.crystal_intensity + 0.07).min(1.0);
                let pitch = 0.5 + self.crystal_intensity * random() * 1.2;
                let volume = 0.1 + self.crystal_intensity * 1.2;
                out.push(StepSound {
                    name: "minecraft:block.amethyst_block.chime",
                    volume,
                    pitch,
                });
                self.last_crystal_tick = input.tick_count;
            }
        }
        true
    }
}

/// `Entity.isStateClimbable`: `#climbable` or powder snow.
fn climbable(f: u16) -> bool {
    f & (flags::CLIMBABLE | flags::POWDER_SNOW) != 0
}

/// `Player.playStepSound`.
fn player_step_sound(
    pos: [i32; 3],
    on: StateSound,
    state: &PlayerState,
    sounds: &dyn Fn(i32, i32, i32) -> StateSound,
    random: &mut dyn FnMut() -> f32,
    out: &mut Vec<StepSound>,
) {
    if state.in_water() {
        out.push(swim_sound(state, random));
        out.push(muffled(on));
        return;
    }
    // `getPrimaryStepSoundBlockPos`: the block above when it is an inside or
    // combination step-sound block (a carpet, snow layer, lily pad).
    let above = sounds(pos[0], pos[1] + 1, pos[2]);
    if above.has(tags::INSIDE_STEP) || above.has(tags::COMBINATION_STEP) {
        if above.has(tags::COMBINATION_STEP) {
            // `playCombinationStepSounds`.
            out.push(step(above));
            out.push(muffled(on));
        } else {
            out.push(step(above));
        }
    } else {
        out.push(step(on));
    }
}

/// `Entity.playStepSound`.
fn step(s: StateSound) -> StepSound {
    StepSound {
        name: s.sound.step_sound,
        volume: s.sound.volume * 0.15,
        pitch: s.sound.pitch,
    }
}

/// `Entity.playMuffledStepSound`.
fn muffled(s: StateSound) -> StepSound {
    StepSound {
        name: s.sound.step_sound,
        volume: s.sound.volume * 0.05,
        pitch: s.sound.pitch * 0.8,
    }
}

/// `Entity.waterSwimSound` for a player not steering a vehicle (0.35), then
/// `playSwimSound` with `Player.getSwimSound`.
fn swim_sound(state: &PlayerState, random: &mut dyn FnMut() -> f32) -> StepSound {
    let (dx, dy, dz) = (state.vx, state.vy, state.vz);
    let speed = ((dx * dx * 0.2f32 as f64 + dy * dy + dz * dz * 0.2f32 as f64).sqrt() as f32
        * 0.35)
        .min(1.0);
    let a = random();
    let b = random();
    StepSound {
        name: "minecraft:entity.player.swim",
        volume: speed,
        pitch: 1.0 + (a - b) * 0.4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_data::block_physics::BlockPhysics;
    use rewo_data::sound_types::block_sound;

    /// Grass at y 0 everywhere, an optional block at `extra`, air elsewhere.
    struct Floor {
        top: &'static str,
        extra: Option<([i32; 3], &'static str)>,
    }

    const CUBE: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

    impl Floor {
        fn name(&self, x: i32, y: i32, z: i32) -> Option<&'static str> {
            match self.extra {
                Some((p, n)) if p == [x, y, z] => Some(n),
                _ if y == 0 => Some(self.top),
                _ => None,
            }
        }
        fn sound(&self, x: i32, y: i32, z: i32) -> StateSound {
            let n = self.name(x, y, z).unwrap_or("minecraft:air");
            let mut t = 0;
            if rewo_data::block_sounds::COMBINATION_STEP_SOUND_BLOCKS.contains(&n) {
                t |= tags::COMBINATION_STEP;
            }
            if rewo_data::block_sounds::CRYSTAL_SOUND_BLOCKS.contains(&n) {
                t |= tags::CRYSTAL;
            }
            StateSound {
                sound: block_sound(n),
                tags: t,
                block: 0,
            }
        }
    }

    impl PhysicsWorld for Floor {
        fn collision(&self, _x: i32, y: i32, _z: i32) -> &[[f32; 6]] {
            if y == 0 {
                CUBE
            } else {
                &[]
            }
        }
        fn block(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
            match self.name(x, y, z) {
                Some(_) => BlockPhysics::default(),
                None => BlockPhysics::AIR,
            }
        }
        fn has_chunk(&self, _x: i32, _z: i32) -> bool {
            true
        }
        fn min_y(&self) -> i32 {
            -64
        }
    }

    fn walker() -> PlayerState {
        let mut s = PlayerState::at(0.5, 1.0, 0.5);
        s.on_ground = true;
        s.supporting_block = Some([0, 0, 0]);
        s
    }

    fn input(moved: [f64; 3]) -> StepInput {
        StepInput {
            moved,
            flying: false,
            shift_down: false,
            passenger: false,
            silent: false,
            tick_count: 100,
        }
    }

    /// Walk `ticks` ticks of `per_tick` blocks along x and collect the sounds.
    fn walk(floor: &Floor, ticks: usize, per_tick: f64, inp: StepInput) -> Vec<(usize, StepSound)> {
        let mut t = StepTracker::default();
        let mut s = walker();
        let mut out = Vec::new();
        for i in 0..ticks {
            s.x += per_tick;
            s.supporting_block = Some([s.x.floor() as i32, 0, 0]);
            let snd = |x, y, z| floor.sound(x, y, z);
            let mut r = || 0.5f32;
            let inp = StepInput {
                moved: [per_tick, 0.0, 0.0],
                ..inp
            };
            for e in t.tick(&s, &inp, floor, &snd, &mut r) {
                out.push((i, e));
            }
        }
        out
    }

    #[test]
    fn a_walk_steps_once_per_block_of_scaled_distance() {
        let floor = Floor {
            top: "minecraft:grass_block",
            extra: None,
        };
        // 0.2 blocks a tick is 0.12 of moveDist a tick (`* 0.6F`): the first
        // step once moveDist passes 1.0 (tick 8, 0.96 -> 1.08), then one per
        // whole unit after.
        let got = walk(&floor, 40, 0.2, input([0.0; 3]));
        let ticks: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
        assert_eq!(ticks[0], 8);
        assert_eq!(
            got.len(),
            4,
            "40 ticks x 0.12 = 4.8 units: steps past 1, 2, 3, 4"
        );
        let (_, s) = got[0];
        assert_eq!(s.name, "minecraft:block.grass.step");
        assert_eq!(s.volume, 1.0 * 0.15);
        assert_eq!(s.pitch, 1.0);
    }

    #[test]
    fn sneaking_flying_and_riding_are_silent() {
        let floor = Floor {
            top: "minecraft:stone",
            extra: None,
        };
        for inp in [
            StepInput {
                shift_down: true,
                ..input([0.0; 3])
            },
            StepInput {
                flying: true,
                ..input([0.0; 3])
            },
            StepInput {
                passenger: true,
                ..input([0.0; 3])
            },
        ] {
            assert!(walk(&floor, 40, 0.2, inp).is_empty(), "{inp:?}");
        }
        // Silent: the step still counts (no sound), so nothing plays.
        let silent = StepInput {
            silent: true,
            ..input([0.0; 3])
        };
        assert!(walk(&floor, 40, 0.2, silent).is_empty());
    }

    #[test]
    fn a_carpet_plays_its_own_step_and_a_muffled_one_from_below() {
        // A carpet above the planks the player stands on: the carpet's step is
        // the primary one and the planks' plays muffled under it.
        let floor = Floor {
            top: "minecraft:oak_planks",
            extra: None,
        };
        let mut t = StepTracker::default();
        let mut s = walker();
        s.x += 2.0;
        let snd = |x: i32, y: i32, z: i32| {
            if y == 1 {
                StateSound {
                    sound: block_sound("minecraft:red_carpet"),
                    tags: tags::COMBINATION_STEP,
                    block: 1,
                }
            } else {
                floor.sound(x, y, z)
            }
        };
        let mut r = || 0.5f32;
        let out = t.tick(&s, &input([2.0, 0.0, 0.0]), &floor, &snd, &mut r);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "minecraft:block.wool.step");
        assert_eq!(out[0].volume, 1.0 * 0.15);
        assert_eq!(out[1].name, "minecraft:block.wood.step");
        assert_eq!(out[1].volume, 1.0 * 0.05);
        assert_eq!(out[1].pitch, 1.0 * 0.8);
    }

    #[test]
    fn amethyst_chimes_at_most_once_a_second() {
        let floor = Floor {
            top: "minecraft:amethyst_block",
            extra: None,
        };
        let got = walk(&floor, 60, 0.3, input([0.0; 3]));
        let chimes: Vec<_> = got
            .iter()
            .filter(|(_, s)| s.name == "minecraft:block.amethyst_block.chime")
            .collect();
        // The tick count is fixed at 100 in `input`, so after the first chime
        // `tickCount >= last + 20` never holds again.
        assert_eq!(chimes.len(), 1);
        let (_, c) = chimes[0];
        // First chime: intensity 0.07 (0 decayed, + 0.07).
        assert!((c.volume - (0.1 + 0.07 * 1.2)).abs() < 1e-6);
        assert!((c.pitch - (0.5 + 0.07 * 0.5 * 1.2)).abs() < 1e-6);
    }

    #[test]
    fn in_air_nothing_plays_and_distance_still_accrues() {
        let floor = Floor {
            top: "minecraft:stone",
            extra: None,
        };
        let mut t = StepTracker::default();
        let mut s = walker();
        s.y = 5.0;
        s.on_ground = false;
        s.supporting_block = None;
        let snd = |x, y, z| floor.sound(x, y, z);
        let mut r = || 0.5f32;
        let out = t.tick(&s, &input([3.0, 0.0, 0.0]), &floor, &snd, &mut r);
        assert!(out.is_empty());
        assert!((t.move_dist - 3.0 * 0.6f32).abs() < 1e-5);
        assert_eq!(t.next_step, 1.0, "no step taken, so nextStep is unchanged");
    }
}
