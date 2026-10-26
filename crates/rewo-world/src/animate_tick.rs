//! The sound half of `ClientLevel.animateTick`: every client tick vanilla
//! picks 667 random blocks within 16 of the player and 667 within 32, and runs
//! each one's `Block.animateTick` and `FluidState.animateTick`. Those play
//! the world's ambient block sounds (flowing water, lava, fire, a lit furnace,
//! a campfire, a nether portal) with `playLocalSound`: no packet carries them.
//!
//! Only the sound-making classes are transcribed; their particles are not
//! (Rewo has no `animateTick` particles yet).

use crate::biome_noise::LegacyRandom;

/// Which `animateTick` sound a block state can make, from its block and
/// properties ([`emitter_for`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Emitter {
    #[default]
    None,
    /// `WaterFluid.animateTick`: a water state that is neither a source nor
    /// falling (`level` 1..=7).
    FlowingWater,
    /// `LavaFluid.animateTick`, any lava state; needs air above.
    Lava,
    /// `FurnaceBlock` / `BlastFurnaceBlock` / `SmokerBlock` with `lit=true`.
    Furnace,
    BlastFurnace,
    Smoker,
    /// `BaseFireBlock.animateTick` (fire, soul fire).
    Fire,
    /// `CampfireBlock` with `lit=true`.
    Campfire,
    /// `NetherPortalBlock.animateTick`.
    NetherPortal,
}

/// The [`Emitter`] of a block state, from its registry name and property
/// lookup.
pub fn emitter_for(block: &str, prop: &dyn Fn(&str) -> Option<String>) -> Emitter {
    let lit = || prop("lit").as_deref() == Some("true");
    match block.strip_prefix("minecraft:").unwrap_or(block) {
        "water" => {
            // `LiquidBlock`'s `level`: 0 is the source, 8.. falling.
            let level: i32 = prop("level").and_then(|l| l.parse().ok()).unwrap_or(0);
            if (1..8).contains(&level) {
                Emitter::FlowingWater
            } else {
                Emitter::None
            }
        }
        "lava" => Emitter::Lava,
        "furnace" if lit() => Emitter::Furnace,
        "blast_furnace" if lit() => Emitter::BlastFurnace,
        "smoker" if lit() => Emitter::Smoker,
        "fire" | "soul_fire" => Emitter::Fire,
        "campfire" | "soul_campfire" if lit() => Emitter::Campfire,
        "nether_portal" => Emitter::NetherPortal,
        _ => Emitter::None,
    }
}

/// One `playLocalSound` an [`Emitter`] asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientSound {
    pub name: &'static str,
    /// `true`: `SoundSource.AMBIENT`; `false`: `BLOCKS`.
    pub ambient: bool,
    pub pos: [f64; 3],
    pub volume: f32,
    pub pitch: f32,
}

/// The block's `animateTick` sound, drawing from `random` exactly as the
/// class does (draws for particles included, so the stream stays in step).
/// `air_above` is the lava test `getBlockState(above).isAir()`.
pub fn tick_block(
    e: Emitter,
    pos: [i32; 3],
    air_above: bool,
    random: &mut LegacyRandom,
) -> Vec<AmbientSound> {
    let [x, y, z] = pos.map(|v| v as f64);
    let mut out = Vec::new();
    let mut push = |name, ambient, pos, volume, pitch| {
        out.push(AmbientSound {
            name,
            ambient,
            pos,
            volume,
            pitch,
        })
    };
    match e {
        Emitter::None => {}
        Emitter::FlowingWater => {
            if random.next_int(64) == 0 {
                let volume = random.next_float() * 0.25 + 0.75;
                let pitch = random.next_float() + 0.5;
                push(
                    "minecraft:block.water.ambient",
                    true,
                    [x + 0.5, y + 0.5, z + 0.5],
                    volume,
                    pitch,
                );
            }
        }
        Emitter::Lava => {
            if air_above {
                if random.next_int(100) == 0 {
                    let xx = x + random.next_double();
                    let zz = z + random.next_double();
                    let volume = 0.2 + random.next_float() * 0.2;
                    let pitch = 0.9 + random.next_float() * 0.15;
                    push(
                        "minecraft:block.lava.pop",
                        true,
                        [xx, y + 1.0, zz],
                        volume,
                        pitch,
                    );
                }
                if random.next_int(200) == 0 {
                    let volume = 0.2 + random.next_float() * 0.2;
                    let pitch = 0.9 + random.next_float() * 0.15;
                    push(
                        "minecraft:block.lava.ambient",
                        true,
                        [x, y, z],
                        volume,
                        pitch,
                    );
                }
            }
        }
        Emitter::Furnace | Emitter::BlastFurnace | Emitter::Smoker => {
            if random.next_double() < 0.1 {
                let name = match e {
                    Emitter::Furnace => "minecraft:block.furnace.fire_crackle",
                    Emitter::BlastFurnace => "minecraft:block.blastfurnace.fire_crackle",
                    _ => "minecraft:block.smoker.smoke",
                };
                push(name, false, [x + 0.5, y, z + 0.5], 1.0, 1.0);
            }
        }
        Emitter::Fire => {
            if random.next_int(24) == 0 {
                let volume = 1.0 + random.next_float();
                let pitch = random.next_float() * 0.7 + 0.3;
                push(
                    "minecraft:block.fire.ambient",
                    false,
                    [x + 0.5, y + 0.5, z + 0.5],
                    volume,
                    pitch,
                );
            }
        }
        Emitter::Campfire => {
            if random.next_int(10) == 0 {
                let volume = 0.5 + random.next_float();
                let pitch = random.next_float() * 0.7 + 0.6;
                push(
                    "minecraft:block.campfire.crackle",
                    false,
                    [x + 0.5, y + 0.5, z + 0.5],
                    volume,
                    pitch,
                );
            }
        }
        Emitter::NetherPortal => {
            if random.next_int(100) == 0 {
                let pitch = random.next_float() * 0.4 + 0.8;
                push(
                    "minecraft:block.portal.ambient",
                    false,
                    [x + 0.5, y + 0.5, z + 0.5],
                    0.5,
                    pitch,
                );
            }
        }
    }
    out
}

/// `ClientLevel.animateTick(xt, yt, zt)`'s positions: 667 rounds of one
/// block within 16 and one within 32 (`xt + nextInt(r) - nextInt(r)` per
/// axis), in vanilla's order. `visit` gets each position.
pub fn sample_positions(
    center: [i32; 3],
    random: &mut LegacyRandom,
    visit: &mut dyn FnMut([i32; 3], &mut LegacyRandom),
) {
    for _ in 0..667 {
        for r in [16, 32] {
            let mut p = [0; 3];
            for (a, c) in p.iter_mut().zip(center) {
                *a = c + random.next_int(r) - random.next_int(r);
            }
            visit(p, random);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            list.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn emitters_follow_the_states_vanilla_tests() {
        assert_eq!(
            emitter_for("minecraft:water", &props(&[("level", "0")])),
            Emitter::None
        );
        assert_eq!(
            emitter_for("minecraft:water", &props(&[("level", "3")])),
            Emitter::FlowingWater
        );
        assert_eq!(
            emitter_for("minecraft:water", &props(&[("level", "8")])),
            Emitter::None
        );
        assert_eq!(
            emitter_for("minecraft:furnace", &props(&[("lit", "false")])),
            Emitter::None
        );
        assert_eq!(
            emitter_for("minecraft:furnace", &props(&[("lit", "true")])),
            Emitter::Furnace
        );
        assert_eq!(
            emitter_for("minecraft:soul_fire", &props(&[])),
            Emitter::Fire
        );
        assert_eq!(
            emitter_for("minecraft:lava", &props(&[("level", "0")])),
            Emitter::Lava
        );
        assert_eq!(emitter_for("minecraft:stone", &props(&[])), Emitter::None);
    }

    /// A lit furnace crackles on about 10% of its visits, at its bottom
    /// centre, BLOCKS, volume and pitch 1; flowing water about 1 in 64.
    #[test]
    fn rates_and_arguments_are_vanillas() {
        let mut r = LegacyRandom::new(7);
        let n = 20_000;
        let mut crackles = 0;
        for _ in 0..n {
            for s in tick_block(Emitter::Furnace, [1, 2, 3], false, &mut r) {
                assert_eq!(s.name, "minecraft:block.furnace.fire_crackle");
                assert_eq!(s.pos, [1.5, 2.0, 3.5]);
                assert_eq!((s.volume, s.pitch, s.ambient), (1.0, 1.0, false));
                crackles += 1;
            }
        }
        let rate = crackles as f64 / n as f64;
        assert!((rate - 0.1).abs() < 0.01, "{rate}");
        let mut water = 0;
        for _ in 0..n {
            for s in tick_block(Emitter::FlowingWater, [0, 0, 0], false, &mut r) {
                assert!(s.ambient);
                assert!((0.75..1.0).contains(&s.volume) && (0.5..1.5).contains(&s.pitch));
                water += 1;
            }
        }
        let rate = water as f64 / n as f64;
        assert!((rate - 1.0 / 64.0).abs() < 0.004, "{rate}");
        // Lava under a block makes nothing.
        for _ in 0..1000 {
            assert!(tick_block(Emitter::Lava, [0, 0, 0], false, &mut r).is_empty());
        }
    }

    #[test]
    fn positions_stay_within_the_radius() {
        let mut r = LegacyRandom::new(1);
        let mut n = 0;
        sample_positions([100, 64, -50], &mut r, &mut |p, _| {
            assert!((p[0] - 100).abs() < 32 && (p[1] - 64).abs() < 32 && (p[2] + 50).abs() < 32);
            n += 1;
        });
        assert_eq!(n, 2 * 667);
    }
}
