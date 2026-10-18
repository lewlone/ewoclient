//! The data survival block breaking reads (M217): each block state's
//! `destroyTime`, `requiresCorrectToolForDrops` and block registry id, and
//! each item's `tool` component — the two generated tables
//! ([`crate::block_hardness`], [`crate::item_tools`]) resolved against the
//! loaded registries.

use std::collections::HashMap;

use crate::item_tools::ToolRule;

/// One block state's mining facts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockMining {
    pub destroy_time: f32,
    pub requires_correct_tool: bool,
    /// The block's `minecraft:block` registry id — what a block tag lists.
    pub block_id: i32,
}

/// One item's prototype `tool` component.
#[derive(Clone, Copy, Debug)]
pub struct ItemTool {
    pub default_speed: f32,
    pub rules: &'static [ToolRule],
}

/// Per-state block facts and per-item tools.
#[derive(Debug, Default)]
pub struct MiningTables {
    states: Vec<Option<BlockMining>>,
    tools: HashMap<i32, ItemTool>,
}

impl MiningTables {
    /// Resolve the generated tables against the registries. A state whose
    /// block the hardness table lacks stays `None` (the app then does not
    /// mine it), so a version drift shows as unbreakable, not as instant.
    pub fn build(
        blocks: &crate::blocks::Blocks,
        block_ids: &crate::stats::IdTable,
        items: &crate::items::Items,
    ) -> Self {
        let hardness: HashMap<&str, (f32, bool)> = crate::block_hardness::BLOCK_HARDNESS
            .iter()
            .map(|&(n, t, r)| (n, (t, r)))
            .collect();
        let states = (0..blocks.state_count() as u32)
            .map(|s| {
                let name = blocks.block_name(s)?;
                let (destroy_time, requires_correct_tool) = *hardness.get(name)?;
                Some(BlockMining {
                    destroy_time,
                    requires_correct_tool,
                    block_id: block_ids.id_of(name)?,
                })
            })
            .collect();
        let tools = crate::item_tools::ITEM_TOOLS
            .iter()
            .filter_map(|&(name, default_speed, rules)| {
                Some((
                    items.id(name)?,
                    ItemTool {
                        default_speed,
                        rules,
                    },
                ))
            })
            .collect();
        Self { states, tools }
    }

    pub fn block(&self, state: u32) -> Option<BlockMining> {
        self.states.get(state as usize).copied().flatten()
    }

    pub fn tool(&self, item_id: i32) -> Option<ItemTool> {
        self.tools.get(&item_id).copied()
    }
}

/// Whether a rule's `blocks` names this block: `#tag` asks `in_tag(tag,
/// block_id)` (the server's `update_tags`), otherwise the `|`-joined ids.
pub fn rule_matches(
    blocks: &str,
    block_name: &str,
    block_id: i32,
    in_tag: &dyn Fn(&str, i32) -> bool,
) -> bool {
    match blocks.strip_prefix('#') {
        Some(tag) => in_tag(tag, block_id),
        None => blocks.split('|').any(|b| b == block_name),
    }
}

impl ItemTool {
    /// `Tool.getMiningSpeed`: the first rule WITH a speed whose blocks match,
    /// else the default.
    pub fn speed(
        &self,
        block_name: &str,
        block_id: i32,
        in_tag: &dyn Fn(&str, i32) -> bool,
    ) -> f32 {
        self.rules
            .iter()
            .find_map(|&(b, speed, _)| {
                speed.filter(|_| rule_matches(b, block_name, block_id, in_tag))
            })
            .unwrap_or(self.default_speed)
    }

    /// `Tool.isCorrectForDrops`: the first rule WITH the flag whose blocks
    /// match, else false.
    pub fn correct_for_drops(
        &self,
        block_name: &str,
        block_id: i32,
        in_tag: &dyn Fn(&str, i32) -> bool,
    ) -> bool {
        self.rules
            .iter()
            .find_map(|&(b, _, correct)| {
                correct.filter(|_| rule_matches(b, block_name, block_id, in_tag))
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wooden pickaxe's two rules, in order: `incorrect_for_wooden_tool`
    /// denies drops first (iron ore), `mineable/pickaxe` mines at 2 and is
    /// correct (stone); a block in neither is the default speed 1 and not
    /// correct. The sword's cobweb rule is an id, not a tag.
    #[test]
    fn rules_resolve_in_order_like_tool() {
        let wood = crate::item_tools::ITEM_TOOLS
            .iter()
            .find(|t| t.0 == "minecraft:wooden_pickaxe")
            .map(|t| ItemTool {
                default_speed: t.1,
                rules: t.2,
            })
            .expect("wooden pickaxe");
        // Block ids 1 = stone, 2 = iron_ore, 3 = dirt.
        let in_tag = |tag: &str, id: i32| match tag {
            "minecraft:mineable/pickaxe" => id == 1 || id == 2,
            "minecraft:incorrect_for_wooden_tool" => id == 2,
            _ => false,
        };
        assert_eq!(wood.speed("minecraft:stone", 1, &in_tag), 2.0);
        assert!(wood.correct_for_drops("minecraft:stone", 1, &in_tag));
        assert_eq!(wood.speed("minecraft:iron_ore", 2, &in_tag), 2.0);
        assert!(!wood.correct_for_drops("minecraft:iron_ore", 2, &in_tag));
        assert_eq!(wood.speed("minecraft:dirt", 3, &in_tag), 1.0);
        assert!(!wood.correct_for_drops("minecraft:dirt", 3, &in_tag));
        let sword = crate::item_tools::ITEM_TOOLS
            .iter()
            .find(|t| t.0 == "minecraft:diamond_sword")
            .map(|t| ItemTool {
                default_speed: t.1,
                rules: t.2,
            })
            .expect("sword");
        assert_eq!(sword.speed("minecraft:cobweb", 9, &|_, _| false), 15.0);
    }

    /// The generated hardness table carries vanilla's literals.
    #[test]
    fn hardness_literals_are_vanillas() {
        let get = |n: &str| {
            crate::block_hardness::BLOCK_HARDNESS
                .iter()
                .find(|r| r.0 == n)
                .map(|r| (r.1, r.2))
        };
        assert_eq!(get("minecraft:dirt"), Some((0.5, false)));
        assert_eq!(get("minecraft:stone"), Some((1.5, true)));
        assert_eq!(get("minecraft:bedrock"), Some((-1.0, false)));
        assert_eq!(get("minecraft:obsidian"), Some((50.0, true)));
        assert_eq!(get("minecraft:red_wall_banner"), Some((1.0, false)));
    }
}
