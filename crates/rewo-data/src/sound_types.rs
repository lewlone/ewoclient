//! Per-state block sounds: `BlockState.getSoundType()` and the step-sound
//! tags, resolved from the generated [`crate::block_sounds`] tables.
//!
//! The player's own steps, places, breaks and mining hits are played by the
//! client from these (`Entity.playStepSound`, `BlockItem.place`,
//! `MultiPlayerGameMode.continueDestroyBlock`), so no packet carries them.

use std::collections::{HashMap, HashSet};

pub use crate::block_sounds::SoundTypeDef;

/// The step-sound tags a state is in, as bits of [`StateSound::tags`].
pub mod tags {
    /// `#minecraft:inside_step_sound_blocks`.
    pub const INSIDE_STEP: u8 = 1 << 0;
    /// `#minecraft:combination_step_sound_blocks`.
    pub const COMBINATION_STEP: u8 = 1 << 1;
    /// `#minecraft:crystal_sound_blocks`.
    pub const CRYSTAL: u8 = 1 << 2;
}

/// One block state's sound type and step-sound tags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StateSound {
    pub sound: &'static SoundTypeDef,
    pub tags: u8,
    /// The same number for every state of one block, and a different one
    /// for each block (`u32::MAX`: no block name). Not a registry id.
    pub block: u32,
}

impl StateSound {
    pub fn has(&self, tag: u8) -> bool {
        self.tags & tag != 0
    }
}

/// `SoundType.<field>` by its constant name (`"WOOD"`).
pub fn sound_type(field: &str) -> Option<&'static SoundTypeDef> {
    crate::block_sounds::SOUND_TYPES
        .iter()
        .find(|(n, _)| *n == field)
        .map(|(_, d)| d)
}

/// `SoundType.STONE`, the `BlockBehaviour.Properties` default.
pub fn stone() -> &'static SoundTypeDef {
    sound_type("STONE").expect("SoundType.STONE is generated")
}

/// A block's sound type by registry name (`minecraft:oak_log`). Every block
/// the generator did not list is `STONE`, as vanilla's properties default.
pub fn block_sound(name: &str) -> &'static SoundTypeDef {
    crate::block_sounds::BLOCK_SOUNDS
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, f)| sound_type(f))
        .unwrap_or_else(stone)
}

/// State id → [`StateSound`] for every state in `blocks`. A state without a
/// block name (a registry drift) is stone with no tags.
pub fn per_state(blocks: &crate::blocks::Blocks) -> Vec<StateSound> {
    let by_name: HashMap<&str, &'static SoundTypeDef> = crate::block_sounds::BLOCK_SOUNDS
        .iter()
        .filter_map(|&(n, f)| Some((n, sound_type(f)?)))
        .collect();
    let set = |list: &[&'static str]| list.iter().copied().collect::<HashSet<&str>>();
    let inside = set(crate::block_sounds::INSIDE_STEP_SOUND_BLOCKS);
    let combination = set(crate::block_sounds::COMBINATION_STEP_SOUND_BLOCKS);
    let crystal = set(crate::block_sounds::CRYSTAL_SOUND_BLOCKS);
    let stone = stone();
    let mut ordinals: HashMap<&str, u32> = HashMap::new();
    (0..blocks.state_count() as u32)
        .map(|s| {
            let Some(name) = blocks.block_name(s) else {
                return StateSound {
                    sound: stone,
                    tags: 0,
                    block: u32::MAX,
                };
            };
            let next = ordinals.len() as u32;
            let block = *ordinals.entry(name).or_insert(next);
            let mut t = 0;
            if inside.contains(name) {
                t |= tags::INSIDE_STEP;
            }
            if combination.contains(name) {
                t |= tags::COMBINATION_STEP;
            }
            if crystal.contains(name) {
                t |= tags::CRYSTAL;
            }
            StateSound {
                sound: by_name.get(name).copied().unwrap_or(stone),
                tags: t,
                block,
            }
        })
        .collect()
}

/// A `BlockSetType`'s sounds (doors, trapdoors, pressure plates, buttons).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockSetSounds {
    pub door_close: &'static str,
    pub door_open: &'static str,
    pub trapdoor_close: &'static str,
    pub trapdoor_open: &'static str,
    pub plate_off: &'static str,
    pub plate_on: &'static str,
    pub button_off: &'static str,
    pub button_on: &'static str,
}

/// The `BlockSetType` sounds of a block built on one (`minecraft:oak_door`),
/// directly or through its `WoodType`.
pub fn block_set_sounds(name: &str) -> Option<BlockSetSounds> {
    let set = crate::block_sounds::BLOCK_SETS
        .iter()
        .find(|(n, _)| *n == name)?
        .1;
    let (_, _, e) = crate::block_sounds::BLOCK_SET_TYPES
        .iter()
        .find(|(n, _, _)| *n == set)?;
    Some(BlockSetSounds {
        door_close: e[0],
        door_open: e[1],
        trapdoor_close: e[2],
        trapdoor_open: e[3],
        plate_off: e[4],
        plate_on: e[5],
        button_off: e[6],
        button_on: e[7],
    })
}

/// A fence gate's `(close, open)` sounds, from its `WoodType`.
pub fn fence_gate_sounds(name: &str) -> Option<(&'static str, &'static str)> {
    let wood = crate::block_sounds::BLOCK_WOOD_TYPES
        .iter()
        .find(|(n, _)| *n == name)?
        .1;
    crate::block_sounds::WOOD_TYPES
        .iter()
        .find(|(n, _, _, _)| *n == wood)
        .map(|&(_, _, close, open)| (close, open))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expectations are `Blocks.java` / `SoundType.java` literals, read from
    // the decompile, not from the generated table.
    #[test]
    fn blocks_take_their_registered_sound_type() {
        assert_eq!(
            block_sound("minecraft:oak_log").step_sound,
            "minecraft:block.wood.step"
        );
        assert_eq!(
            block_sound("minecraft:grass_block").step_sound,
            "minecraft:block.grass.step"
        );
        assert_eq!(
            block_sound("minecraft:sand").step_sound,
            "minecraft:block.sand.step"
        );
        assert_eq!(
            block_sound("minecraft:white_wool").step_sound,
            "minecraft:block.wool.step"
        );
        assert_eq!(
            block_sound("minecraft:stone").step_sound,
            "minecraft:block.stone.step"
        );
        // Not in the table at all: the `Properties` default.
        assert_eq!(
            block_sound("minecraft:no_such_block").step_sound,
            "minecraft:block.stone.step"
        );
        // `SoundType.ANVIL` is quiet: `new SoundType(0.3F, 1.0F, ...)`.
        let anvil = block_sound("minecraft:anvil");
        assert_eq!((anvil.volume, anvil.pitch), (0.3, 1.0));
        // `SoundType.SNOW`'s step: `SoundEvents.SNOW_STEP`.
        assert_eq!(
            block_sound("minecraft:snow_block").step_sound,
            "minecraft:block.snow.step"
        );
    }

    #[test]
    fn constructor_set_types_override_the_properties() {
        // `DoorBlock(BlockSetType type, ...)` re-sets `.sound(type.soundType())`;
        // `BlockSetType.IRON` names `SoundType.IRON`.
        assert_eq!(
            block_sound("minecraft:iron_door").place_sound,
            "minecraft:block.iron.place"
        );
        assert_eq!(
            block_sound("minecraft:oak_door").place_sound,
            "minecraft:block.wood.place"
        );
        // A hanging sign takes its `WoodType.hangingSignSoundType`.
        assert_eq!(
            block_sound("minecraft:oak_hanging_sign").place_sound,
            "minecraft:block.hanging_sign.place"
        );
        let door = block_set_sounds("minecraft:iron_door").unwrap();
        assert_eq!(door.door_open, "minecraft:block.iron_door.open");
        let oak = block_set_sounds("minecraft:oak_trapdoor").unwrap();
        assert_eq!(oak.trapdoor_close, "minecraft:block.wooden_trapdoor.close");
        assert_eq!(
            fence_gate_sounds("minecraft:oak_fence_gate"),
            Some((
                "minecraft:block.fence_gate.close",
                "minecraft:block.fence_gate.open"
            ))
        );
        assert_eq!(block_set_sounds("minecraft:stone"), None);
    }

    #[test]
    fn step_tags_come_from_the_tag_files() {
        let has = |list: &[&str], n: &str| list.contains(&n);
        use crate::block_sounds::*;
        assert!(
            has(COMBINATION_STEP_SOUND_BLOCKS, "minecraft:red_carpet"),
            "#wool_carpets expands"
        );
        assert!(has(COMBINATION_STEP_SOUND_BLOCKS, "minecraft:snow"));
        assert!(has(INSIDE_STEP_SOUND_BLOCKS, "minecraft:lily_pad"));
        assert!(has(CRYSTAL_SOUND_BLOCKS, "minecraft:amethyst_block"));
    }
}
