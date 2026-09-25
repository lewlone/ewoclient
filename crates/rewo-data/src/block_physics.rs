//! Per-state movement facts the player physics reads beyond collision boxes.
//!
//! Everything here is vanilla block *behaviour* that lives in code
//! (`Properties.friction`, `entityInside`, `#minecraft:climbable`, …); the
//! tables come from `tools/gen_block_props.py` (`crate::block_props`) and are
//! resolved per state by the asset bake.

/// `Properties.friction` default (`BlockBehaviour.Properties.friction = 0.6F`).
pub const DEFAULT_FRICTION: f32 = 0.6;

/// The fluid a state holds, as `EntityFluidInteraction` sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PhysFluid {
    #[default]
    None,
    /// `FluidState.getOwnHeight()` = `amount / 9` — 8/9 for a source or a
    /// falling column (the "same fluid above" 1.0 is resolved by the caller).
    Water { own_height: f32 },
    Lava { own_height: f32 },
}

/// `makeStuckInBlock` source for a block's `entityInside`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Stuck {
    #[default]
    None,
    /// `WebBlock` — unconditional.
    Web,
    /// `SweetBerryBushBlock` — any living entity but a fox or a bee.
    SweetBerryBush,
    /// `PowderSnowBlock` — a living entity only while its feet block is
    /// powder snow (`entity.getInBlockState().is(this)`).
    PowderSnow,
}

impl Stuck {
    /// The multiplier `makeStuckInBlock` stores (`WebBlock`'s no-weaving arm).
    pub fn multiplier(self) -> Option<[f64; 3]> {
        let name = match self {
            Stuck::None => return None,
            Stuck::Web => "minecraft:cobweb",
            Stuck::SweetBerryBush => "minecraft:sweet_berry_bush",
            Stuck::PowderSnow => "minecraft:powder_snow",
        };
        crate::block_props::STUCK
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, m)| *m)
    }
}

/// Flag bits of [`BlockPhysics::flags`].
pub mod flags {
    /// `#minecraft:climbable`.
    pub const CLIMBABLE: u16 = 1 << 0;
    /// `Blocks.SCAFFOLDING` (sliding down is never suppressed on it).
    pub const SCAFFOLDING: u16 = 1 << 1;
    /// A `TrapDoorBlock` with `open=true` (a ladder only above a same-facing
    /// ladder — `LivingEntity.trapdoorUsableAsLadder`).
    pub const OPEN_TRAPDOOR: u16 = 1 << 2;
    /// `Blocks.LADDER`.
    pub const LADDER: u16 = 1 << 3;
    /// `#minecraft:fences`.
    pub const FENCE: u16 = 1 << 4;
    /// `#minecraft:walls`.
    pub const WALL: u16 = 1 << 5;
    /// A `FenceGateBlock`.
    pub const FENCE_GATE: u16 = 1 << 6;
    /// `Blocks.POWDER_SNOW`.
    pub const POWDER_SNOW: u16 = 1 << 7;
    /// `Blocks.WATER` or `Blocks.BUBBLE_COLUMN` — `getBlockSpeedFactor`
    /// never looks below these.
    pub const WATER_BLOCK: u16 = 1 << 8;
    /// `BlockState.isAir()`.
    pub const AIR: u16 = 1 << 9;
    /// `#minecraft:suppresses_bounce`.
    pub const SUPPRESSES_BOUNCE: u16 = 1 << 10;
    /// `Blocks.BUBBLE_COLUMN` with `DRAG_DOWN` false (`drag=false`).
    pub const BUBBLE_COLUMN_UP: u16 = 1 << 11;
    /// `Blocks.BUBBLE_COLUMN` with `DRAG_DOWN` true (`drag=true`).
    pub const BUBBLE_COLUMN_DOWN: u16 = 1 << 12;
    /// `Blocks.HONEY_BLOCK`.
    pub const HONEY: u16 = 1 << 13;
}

/// Per-state movement behaviour. `Default` is plain stone: friction 0.6,
/// factors 1.0, no flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockPhysics {
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    /// `Properties.bounceRestitution` (slime 1.0, beds 0.75).
    pub bounce: f32,
    pub flags: u16,
    /// Horizontal `facing` as a 2D data value (0 south, 1 west, 2 north,
    /// 3 east — `Direction.get2DDataValue`), or 255 when the state has none.
    pub facing: u8,
    pub stuck: Stuck,
    pub fluid: PhysFluid,
}

impl Default for BlockPhysics {
    fn default() -> Self {
        Self {
            friction: DEFAULT_FRICTION,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce: 0.0,
            flags: 0,
            facing: 255,
            stuck: Stuck::None,
            fluid: PhysFluid::None,
        }
    }
}

impl BlockPhysics {
    /// Air as the physics sees it (an unloaded or out-of-range cell).
    pub const AIR: BlockPhysics = BlockPhysics {
        friction: DEFAULT_FRICTION,
        speed_factor: 1.0,
        jump_factor: 1.0,
        bounce: 0.0,
        flags: flags::AIR,
        facing: 255,
        stuck: Stuck::None,
        fluid: PhysFluid::None,
    };

    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// Resolve one state from its block name and properties.
    pub fn resolve(
        block: &str,
        props: Option<&serde_json::Map<String, serde_json::Value>>,
        water_block: bool,
    ) -> Self {
        use crate::block_props as bp;
        let find = |t: &[(&str, f32)], d: f32| t.iter().find(|(n, _)| *n == block).map_or(d, |(_, v)| *v);
        let has = |t: &[&str]| t.contains(&block);
        let prop = |name: &str| props.and_then(|p| p.get(name)).and_then(|v| v.as_str());
        let mut f = 0u16;
        if has(bp::CLIMBABLE_TAG) {
            f |= flags::CLIMBABLE;
        }
        if block == "minecraft:scaffolding" {
            f |= flags::SCAFFOLDING;
        }
        if has(bp::TRAPDOORS) && prop("open") == Some("true") {
            f |= flags::OPEN_TRAPDOOR;
        }
        if block == "minecraft:ladder" {
            f |= flags::LADDER;
        }
        if has(bp::FENCES_TAG) {
            f |= flags::FENCE;
        }
        if has(bp::WALLS_TAG) {
            f |= flags::WALL;
        }
        if has(bp::FENCE_GATES) {
            f |= flags::FENCE_GATE;
        }
        if block == "minecraft:powder_snow" {
            f |= flags::POWDER_SNOW;
        }
        if block == "minecraft:bubble_column" {
            // `DRAG_DOWN` is registered true (`BubbleColumnBlock`'s default
            // state), so an absent `drag` is the drag-down column.
            f |= if prop("drag") == Some("false") {
                flags::BUBBLE_COLUMN_UP
            } else {
                flags::BUBBLE_COLUMN_DOWN
            };
        }
        if block == "minecraft:honey_block" {
            f |= flags::HONEY;
        }
        if water_block {
            f |= flags::WATER_BLOCK;
        }
        if has(bp::SUPPRESSES_BOUNCE_TAG) {
            f |= flags::SUPPRESSES_BOUNCE;
        }
        if matches!(block, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air") {
            f |= flags::AIR;
        }
        let facing = match prop("facing") {
            Some("south") => 0,
            Some("west") => 1,
            Some("north") => 2,
            Some("east") => 3,
            _ => 255,
        };
        let stuck = match block {
            "minecraft:cobweb" => Stuck::Web,
            "minecraft:sweet_berry_bush" => Stuck::SweetBerryBush,
            "minecraft:powder_snow" => Stuck::PowderSnow,
            _ => Stuck::None,
        };
        BlockPhysics {
            friction: find(bp::FRICTION, DEFAULT_FRICTION),
            speed_factor: find(bp::SPEED_FACTOR, 1.0),
            jump_factor: find(bp::JUMP_FACTOR, 1.0),
            bounce: find(bp::BOUNCE, 0.0),
            flags: f,
            facing,
            stuck,
            fluid: PhysFluid::None,
        }
    }
}

/// `FluidState.getOwnHeight()` for a `LiquidBlock` `level` property:
/// `stateCache[min(level, 8)]` — 0 is the source (amount 8), 1..7 flow with
/// amount `8 - level`, 8+ is falling with amount 8.
pub fn fluid_own_height(level: u8) -> f32 {
    let amount = if level == 0 || level >= 8 { 8 } else { 8 - level };
    amount as f32 / 9.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_resolve_vanilla_values() {
        assert_eq!(BlockPhysics::resolve("minecraft:ice", None, false).friction, 0.98);
        assert_eq!(BlockPhysics::resolve("minecraft:blue_ice", None, false).friction, 0.989);
        assert_eq!(BlockPhysics::resolve("minecraft:slime_block", None, false).friction, 0.8);
        assert_eq!(BlockPhysics::resolve("minecraft:stone", None, false).friction, 0.6);
        let honey = BlockPhysics::resolve("minecraft:honey_block", None, false);
        assert_eq!((honey.speed_factor, honey.jump_factor), (0.4, 0.5));
        assert_eq!(BlockPhysics::resolve("minecraft:soul_sand", None, false).speed_factor, 0.4);
        assert!(BlockPhysics::resolve("minecraft:ladder", None, false).has(flags::CLIMBABLE));
        assert!(BlockPhysics::resolve("minecraft:twisting_vines_plant", None, false).has(flags::CLIMBABLE));
        assert!(BlockPhysics::resolve("minecraft:oak_fence", None, false).has(flags::FENCE));
        assert!(BlockPhysics::resolve("minecraft:cobblestone_wall", None, false).has(flags::WALL));
        assert_eq!(Stuck::Web.multiplier(), Some([0.25, 0.05f32 as f64, 0.25]));
        assert_eq!(
            Stuck::PowderSnow.multiplier(),
            Some([0.9f32 as f64, 1.5, 0.9f32 as f64])
        );
    }

    #[test]
    fn fluid_heights_follow_the_state_cache() {
        assert_eq!(fluid_own_height(0), 8.0 / 9.0);
        assert_eq!(fluid_own_height(1), 7.0 / 9.0);
        assert_eq!(fluid_own_height(7), 1.0 / 9.0);
        assert_eq!(fluid_own_height(8), 8.0 / 9.0);
        assert_eq!(fluid_own_height(15), 8.0 / 9.0);
    }

    #[test]
    fn bubble_column_drag_and_honey_resolve_to_flags() {
        let drag = |value: &str| {
            let mut p = serde_json::Map::new();
            p.insert("drag".to_string(), value.into());
            p
        };
        let up = BlockPhysics::resolve("minecraft:bubble_column", Some(&drag("false")), true);
        assert!(up.has(flags::BUBBLE_COLUMN_UP) && !up.has(flags::BUBBLE_COLUMN_DOWN));
        let down = BlockPhysics::resolve("minecraft:bubble_column", Some(&drag("true")), true);
        assert!(down.has(flags::BUBBLE_COLUMN_DOWN) && !down.has(flags::BUBBLE_COLUMN_UP));
        // `DRAG_DOWN` is registered true, so a state without `drag` is down.
        assert!(BlockPhysics::resolve("minecraft:bubble_column", None, true).has(flags::BUBBLE_COLUMN_DOWN));
        assert!(BlockPhysics::resolve("minecraft:honey_block", None, false).has(flags::HONEY));
        // And no other block gains them.
        for name in ["minecraft:water", "minecraft:stone", "minecraft:soul_sand"] {
            let b = BlockPhysics::resolve(name, None, true);
            assert!(
                !b.has(flags::BUBBLE_COLUMN_UP | flags::BUBBLE_COLUMN_DOWN | flags::HONEY),
                "{name}"
            );
        }
    }
}

/// The real bake's culling + movement facts. Needs the user's client jar and
/// datagen reports, so it is opt-in:
/// `cargo test -p rewo-data -- --ignored real_bake`.
#[cfg(test)]
mod real_bake {
    use crate::assets::{bake, RenderKind};

    #[test]
    #[ignore]
    fn real_bake_cull_and_physics() {
        let root = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("EwoClient");
        let jar = root.join("shared/versions/26.2/26.2.jar");
        let blocks = root.join("rewo/26.2/datagen/generated/reports/blocks.json");
        let b = bake(&jar, &blocks).expect("bake");
        let table = crate::blocks::Blocks::load(&blocks).expect("blocks");
        let id = |name: &str| table.default_state(name).unwrap_or_else(|| panic!("{name}")) as usize;
        for (name, occludes, translucent, skip) in [
            ("minecraft:stone", 0b11_1111, false, 0),
            ("minecraft:grass_block", 0b11_1111, false, 0),
            ("minecraft:glass", 0, true, 1),
            ("minecraft:white_stained_glass", 0, true, 1),
            ("minecraft:ice", 0, true, 1),
            ("minecraft:oak_leaves", 0, false, 0),
            ("minecraft:slime_block", 0, true, 1),
            ("minecraft:glass_pane", 0, true, 2),
            ("minecraft:iron_bars", 0, false, 2),
        ] {
            let c = b.cull[id(name)];
            assert_eq!((c.occludes, c.skip), (occludes, skip), "{name}: {c:?}");
            let t = match &b.render[id(name)] {
                RenderKind::Cube { .. } => c.translucent,
                RenderKind::Model(m) => b.models[*m as usize].iter().any(|q| q.translucent),
                _ => false,
            };
            assert_eq!(t, translucent, "{name} translucent");
        }
        let full = vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let p = 1.0 / 16.0;
        for (name, want) in [
            ("minecraft:stone", full.clone()),
            ("minecraft:grass_block", full.clone()),
            ("minecraft:glass", full.clone()),
            ("minecraft:soul_sand", vec![[0.0, 0.0, 0.0, 1.0, 14.0 * p, 1.0]]),
            ("minecraft:mud", vec![[0.0, 0.0, 0.0, 1.0, 14.0 * p, 1.0]]),
            ("minecraft:honey_block", vec![[p, 0.0, p, 15.0 * p, 15.0 * p, 15.0 * p]]),
            ("minecraft:chest", vec![[p, 0.0, p, 15.0 * p, 14.0 * p, 15.0 * p]]),
            ("minecraft:skeleton_skull", vec![[0.25, 0.0, 0.25, 0.75, 0.5, 0.75]]),
            ("minecraft:powder_snow", vec![]),
            ("minecraft:torch", vec![]),
            ("minecraft:cobweb", vec![]),
            ("minecraft:snow", vec![]),
        ] {
            assert_eq!(b.collide[id(name)], want, "{name}");
        }
        for name in [
            "minecraft:glass_pane",
            "minecraft:iron_bars",
            "minecraft:lantern",
            "minecraft:iron_chain",
            "minecraft:end_rod",
            "minecraft:bell",
            "minecraft:candle",
            "minecraft:pointed_dripstone",
            "minecraft:red_bed",
            "minecraft:flower_pot",
            "minecraft:campfire",
            "minecraft:brewing_stand",
        ] {
            assert!(!b.collide[id(name)].is_empty(), "{name} collides");
        }
        let fence = &b.collide[id("minecraft:oak_fence")];
        assert!(fence.iter().any(|b| b[4] == 1.5), "fence is 1.5 tall: {fence:?}");
        assert_eq!(b.physics[id("minecraft:ice")].friction, 0.98);
        assert!(b.physics[id("minecraft:ladder")].has(super::flags::CLIMBABLE));
        assert!(matches!(
            b.physics[id("minecraft:water")].fluid,
            super::PhysFluid::Water { .. }
        ));
    }
}
