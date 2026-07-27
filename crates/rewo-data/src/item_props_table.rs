//! Per-item stack size and equippable slot — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_item_props.py` after a version bump.
//!
//! Source: the datagen per-item component report for 26.2. Of 1537
//! items, 295 differ from `Item.DEFAULT_MAX_STACK_SIZE` = 64
//! (246 at 1, 49 at 16) and 84 carry `minecraft:equippable`
//! (44 body, 8 chest, 7 feet, 16 head, 7 legs, 1 offhand, 1 saddle). Items that are neither are not listed.
//!
//! Both feed the container click arithmetic. A wrong cap or a wrongly-allowed
//! armour placement predicts a wrong slot, the server's `HashedStack.matches`
//! fails, and the container resynchronises — which looks like a click that
//! bounced.

/// `EquipmentSlot`, restricted to the members an item can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquipSlot {
    Head,
    Chest,
    Legs,
    Feet,
    Mainhand,
    Offhand,
    Body,
    Saddle,
}

/// `Item.DEFAULT_MAX_STACK_SIZE`, read from the decompile rather than assumed,
/// because the table below is a delta from it.
pub const DEFAULT_MAX_STACK: i32 = 64;

/// `Item.ABSOLUTE_MAX_STACK_SIZE` — the ceiling `stacksTo` asserts against.
pub const ABSOLUTE_MAX_STACK: i32 = 99;

/// Every item that is not (default stack size, not equippable), sorted by name
/// so a binary search finds it. A `None` size means the default.
pub const ITEM_PROPS: &[(&str, Option<i32>, Option<EquipSlot>)] = &[
    ("minecraft:acacia_boat", Some(1), None),
    ("minecraft:acacia_chest_boat", Some(1), None),
    ("minecraft:acacia_hanging_sign", Some(16), None),
    ("minecraft:acacia_sign", Some(16), None),
    ("minecraft:armor_stand", Some(16), None),
    ("minecraft:axolotl_bucket", Some(1), None),
    ("minecraft:bamboo_chest_raft", Some(1), None),
    ("minecraft:bamboo_hanging_sign", Some(16), None),
    ("minecraft:bamboo_raft", Some(1), None),
    ("minecraft:bamboo_sign", Some(16), None),
    ("minecraft:beetroot_soup", Some(1), None),
    ("minecraft:birch_boat", Some(1), None),
    ("minecraft:birch_chest_boat", Some(1), None),
    ("minecraft:birch_hanging_sign", Some(16), None),
    ("minecraft:birch_sign", Some(16), None),
    ("minecraft:black_banner", Some(16), None),
    ("minecraft:black_bed", Some(1), None),
    ("minecraft:black_bundle", Some(1), None),
    ("minecraft:black_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:black_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:black_shulker_box", Some(1), None),
    ("minecraft:blue_banner", Some(16), None),
    ("minecraft:blue_bed", Some(1), None),
    ("minecraft:blue_bundle", Some(1), None),
    ("minecraft:blue_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:blue_egg", Some(16), None),
    ("minecraft:blue_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:blue_shulker_box", Some(1), None),
    ("minecraft:bordure_indented_banner_pattern", Some(1), None),
    ("minecraft:bow", Some(1), None),
    ("minecraft:brown_banner", Some(16), None),
    ("minecraft:brown_bed", Some(1), None),
    ("minecraft:brown_bundle", Some(1), None),
    ("minecraft:brown_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:brown_egg", Some(16), None),
    ("minecraft:brown_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:brown_shulker_box", Some(1), None),
    ("minecraft:brush", Some(1), None),
    ("minecraft:bucket", Some(16), None),
    ("minecraft:bundle", Some(1), None),
    ("minecraft:cake", Some(1), None),
    ("minecraft:carrot_on_a_stick", Some(1), None),
    ("minecraft:carved_pumpkin", None, Some(EquipSlot::Head)),
    ("minecraft:chainmail_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:chainmail_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:chainmail_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:chainmail_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:cherry_boat", Some(1), None),
    ("minecraft:cherry_chest_boat", Some(1), None),
    ("minecraft:cherry_hanging_sign", Some(16), None),
    ("minecraft:cherry_sign", Some(16), None),
    ("minecraft:chest_minecart", Some(1), None),
    ("minecraft:cod_bucket", Some(1), None),
    ("minecraft:command_block_minecart", Some(1), None),
    ("minecraft:copper_axe", Some(1), None),
    ("minecraft:copper_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:copper_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:copper_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:copper_hoe", Some(1), None),
    ("minecraft:copper_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:copper_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:copper_nautilus_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:copper_pickaxe", Some(1), None),
    ("minecraft:copper_shovel", Some(1), None),
    ("minecraft:copper_spear", Some(1), None),
    ("minecraft:copper_sword", Some(1), None),
    ("minecraft:creeper_banner_pattern", Some(1), None),
    ("minecraft:creeper_head", None, Some(EquipSlot::Head)),
    ("minecraft:crimson_hanging_sign", Some(16), None),
    ("minecraft:crimson_sign", Some(16), None),
    ("minecraft:crossbow", Some(1), None),
    ("minecraft:cyan_banner", Some(16), None),
    ("minecraft:cyan_bed", Some(1), None),
    ("minecraft:cyan_bundle", Some(1), None),
    ("minecraft:cyan_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:cyan_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:cyan_shulker_box", Some(1), None),
    ("minecraft:dark_oak_boat", Some(1), None),
    ("minecraft:dark_oak_chest_boat", Some(1), None),
    ("minecraft:dark_oak_hanging_sign", Some(16), None),
    ("minecraft:dark_oak_sign", Some(16), None),
    ("minecraft:debug_stick", Some(1), None),
    ("minecraft:diamond_axe", Some(1), None),
    ("minecraft:diamond_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:diamond_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:diamond_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:diamond_hoe", Some(1), None),
    ("minecraft:diamond_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:diamond_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:diamond_nautilus_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:diamond_pickaxe", Some(1), None),
    ("minecraft:diamond_shovel", Some(1), None),
    ("minecraft:diamond_spear", Some(1), None),
    ("minecraft:diamond_sword", Some(1), None),
    ("minecraft:dragon_head", None, Some(EquipSlot::Head)),
    ("minecraft:egg", Some(16), None),
    ("minecraft:elytra", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:enchanted_book", Some(1), None),
    ("minecraft:ender_pearl", Some(16), None),
    ("minecraft:field_masoned_banner_pattern", Some(1), None),
    ("minecraft:fishing_rod", Some(1), None),
    ("minecraft:flint_and_steel", Some(1), None),
    ("minecraft:flow_banner_pattern", Some(1), None),
    ("minecraft:flower_banner_pattern", Some(1), None),
    ("minecraft:furnace_minecart", Some(1), None),
    ("minecraft:globe_banner_pattern", Some(1), None),
    ("minecraft:goat_horn", Some(1), None),
    ("minecraft:golden_axe", Some(1), None),
    ("minecraft:golden_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:golden_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:golden_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:golden_hoe", Some(1), None),
    ("minecraft:golden_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:golden_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:golden_nautilus_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:golden_pickaxe", Some(1), None),
    ("minecraft:golden_shovel", Some(1), None),
    ("minecraft:golden_spear", Some(1), None),
    ("minecraft:golden_sword", Some(1), None),
    ("minecraft:gray_banner", Some(16), None),
    ("minecraft:gray_bed", Some(1), None),
    ("minecraft:gray_bundle", Some(1), None),
    ("minecraft:gray_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:gray_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:gray_shulker_box", Some(1), None),
    ("minecraft:green_banner", Some(16), None),
    ("minecraft:green_bed", Some(1), None),
    ("minecraft:green_bundle", Some(1), None),
    ("minecraft:green_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:green_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:green_shulker_box", Some(1), None),
    ("minecraft:guster_banner_pattern", Some(1), None),
    ("minecraft:honey_bottle", Some(16), None),
    ("minecraft:hopper_minecart", Some(1), None),
    ("minecraft:iron_axe", Some(1), None),
    ("minecraft:iron_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:iron_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:iron_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:iron_hoe", Some(1), None),
    ("minecraft:iron_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:iron_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:iron_nautilus_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:iron_pickaxe", Some(1), None),
    ("minecraft:iron_shovel", Some(1), None),
    ("minecraft:iron_spear", Some(1), None),
    ("minecraft:iron_sword", Some(1), None),
    ("minecraft:jungle_boat", Some(1), None),
    ("minecraft:jungle_chest_boat", Some(1), None),
    ("minecraft:jungle_hanging_sign", Some(16), None),
    ("minecraft:jungle_sign", Some(16), None),
    ("minecraft:knowledge_book", Some(1), None),
    ("minecraft:lava_bucket", Some(1), None),
    ("minecraft:leather_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:leather_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:leather_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:leather_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:leather_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:light_blue_banner", Some(16), None),
    ("minecraft:light_blue_bed", Some(1), None),
    ("minecraft:light_blue_bundle", Some(1), None),
    ("minecraft:light_blue_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:light_blue_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:light_blue_shulker_box", Some(1), None),
    ("minecraft:light_gray_banner", Some(16), None),
    ("minecraft:light_gray_bed", Some(1), None),
    ("minecraft:light_gray_bundle", Some(1), None),
    ("minecraft:light_gray_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:light_gray_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:light_gray_shulker_box", Some(1), None),
    ("minecraft:lime_banner", Some(16), None),
    ("minecraft:lime_bed", Some(1), None),
    ("minecraft:lime_bundle", Some(1), None),
    ("minecraft:lime_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:lime_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:lime_shulker_box", Some(1), None),
    ("minecraft:lingering_potion", Some(1), None),
    ("minecraft:mace", Some(1), None),
    ("minecraft:magenta_banner", Some(16), None),
    ("minecraft:magenta_bed", Some(1), None),
    ("minecraft:magenta_bundle", Some(1), None),
    ("minecraft:magenta_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:magenta_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:magenta_shulker_box", Some(1), None),
    ("minecraft:mangrove_boat", Some(1), None),
    ("minecraft:mangrove_chest_boat", Some(1), None),
    ("minecraft:mangrove_hanging_sign", Some(16), None),
    ("minecraft:mangrove_sign", Some(16), None),
    ("minecraft:milk_bucket", Some(1), None),
    ("minecraft:minecart", Some(1), None),
    ("minecraft:mojang_banner_pattern", Some(1), None),
    ("minecraft:mushroom_stew", Some(1), None),
    ("minecraft:music_disc_11", Some(1), None),
    ("minecraft:music_disc_13", Some(1), None),
    ("minecraft:music_disc_5", Some(1), None),
    ("minecraft:music_disc_blocks", Some(1), None),
    ("minecraft:music_disc_bounce", Some(1), None),
    ("minecraft:music_disc_cat", Some(1), None),
    ("minecraft:music_disc_chirp", Some(1), None),
    ("minecraft:music_disc_creator", Some(1), None),
    ("minecraft:music_disc_creator_music_box", Some(1), None),
    ("minecraft:music_disc_far", Some(1), None),
    ("minecraft:music_disc_lava_chicken", Some(1), None),
    ("minecraft:music_disc_mall", Some(1), None),
    ("minecraft:music_disc_mellohi", Some(1), None),
    ("minecraft:music_disc_otherside", Some(1), None),
    ("minecraft:music_disc_pigstep", Some(1), None),
    ("minecraft:music_disc_precipice", Some(1), None),
    ("minecraft:music_disc_relic", Some(1), None),
    ("minecraft:music_disc_stal", Some(1), None),
    ("minecraft:music_disc_strad", Some(1), None),
    ("minecraft:music_disc_tears", Some(1), None),
    ("minecraft:music_disc_wait", Some(1), None),
    ("minecraft:music_disc_ward", Some(1), None),
    ("minecraft:netherite_axe", Some(1), None),
    ("minecraft:netherite_boots", Some(1), Some(EquipSlot::Feet)),
    ("minecraft:netherite_chestplate", Some(1), Some(EquipSlot::Chest)),
    ("minecraft:netherite_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:netherite_hoe", Some(1), None),
    ("minecraft:netherite_horse_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:netherite_leggings", Some(1), Some(EquipSlot::Legs)),
    ("minecraft:netherite_nautilus_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:netherite_pickaxe", Some(1), None),
    ("minecraft:netherite_shovel", Some(1), None),
    ("minecraft:netherite_spear", Some(1), None),
    ("minecraft:netherite_sword", Some(1), None),
    ("minecraft:oak_boat", Some(1), None),
    ("minecraft:oak_chest_boat", Some(1), None),
    ("minecraft:oak_hanging_sign", Some(16), None),
    ("minecraft:oak_sign", Some(16), None),
    ("minecraft:orange_banner", Some(16), None),
    ("minecraft:orange_bed", Some(1), None),
    ("minecraft:orange_bundle", Some(1), None),
    ("minecraft:orange_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:orange_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:orange_shulker_box", Some(1), None),
    ("minecraft:pale_oak_boat", Some(1), None),
    ("minecraft:pale_oak_chest_boat", Some(1), None),
    ("minecraft:pale_oak_hanging_sign", Some(16), None),
    ("minecraft:pale_oak_sign", Some(16), None),
    ("minecraft:piglin_banner_pattern", Some(1), None),
    ("minecraft:piglin_head", None, Some(EquipSlot::Head)),
    ("minecraft:pink_banner", Some(16), None),
    ("minecraft:pink_bed", Some(1), None),
    ("minecraft:pink_bundle", Some(1), None),
    ("minecraft:pink_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:pink_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:pink_shulker_box", Some(1), None),
    ("minecraft:player_head", None, Some(EquipSlot::Head)),
    ("minecraft:potion", Some(1), None),
    ("minecraft:powder_snow_bucket", Some(1), None),
    ("minecraft:pufferfish_bucket", Some(1), None),
    ("minecraft:purple_banner", Some(16), None),
    ("minecraft:purple_bed", Some(1), None),
    ("minecraft:purple_bundle", Some(1), None),
    ("minecraft:purple_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:purple_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:purple_shulker_box", Some(1), None),
    ("minecraft:rabbit_stew", Some(1), None),
    ("minecraft:red_banner", Some(16), None),
    ("minecraft:red_bed", Some(1), None),
    ("minecraft:red_bundle", Some(1), None),
    ("minecraft:red_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:red_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:red_shulker_box", Some(1), None),
    ("minecraft:saddle", Some(1), Some(EquipSlot::Saddle)),
    ("minecraft:salmon_bucket", Some(1), None),
    ("minecraft:shears", Some(1), None),
    ("minecraft:shield", Some(1), Some(EquipSlot::Offhand)),
    ("minecraft:shulker_box", Some(1), None),
    ("minecraft:skeleton_skull", None, Some(EquipSlot::Head)),
    ("minecraft:skull_banner_pattern", Some(1), None),
    ("minecraft:snowball", Some(16), None),
    ("minecraft:splash_potion", Some(1), None),
    ("minecraft:spruce_boat", Some(1), None),
    ("minecraft:spruce_chest_boat", Some(1), None),
    ("minecraft:spruce_hanging_sign", Some(16), None),
    ("minecraft:spruce_sign", Some(16), None),
    ("minecraft:spyglass", Some(1), None),
    ("minecraft:stone_axe", Some(1), None),
    ("minecraft:stone_hoe", Some(1), None),
    ("minecraft:stone_pickaxe", Some(1), None),
    ("minecraft:stone_shovel", Some(1), None),
    ("minecraft:stone_spear", Some(1), None),
    ("minecraft:stone_sword", Some(1), None),
    ("minecraft:sulfur_cube_bucket", Some(1), None),
    ("minecraft:suspicious_stew", Some(1), None),
    ("minecraft:tadpole_bucket", Some(1), None),
    ("minecraft:tnt_minecart", Some(1), None),
    ("minecraft:totem_of_undying", Some(1), None),
    ("minecraft:trident", Some(1), None),
    ("minecraft:tropical_fish_bucket", Some(1), None),
    ("minecraft:turtle_helmet", Some(1), Some(EquipSlot::Head)),
    ("minecraft:warped_fungus_on_a_stick", Some(1), None),
    ("minecraft:warped_hanging_sign", Some(16), None),
    ("minecraft:warped_sign", Some(16), None),
    ("minecraft:water_bucket", Some(1), None),
    ("minecraft:white_banner", Some(16), None),
    ("minecraft:white_bed", Some(1), None),
    ("minecraft:white_bundle", Some(1), None),
    ("minecraft:white_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:white_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:white_shulker_box", Some(1), None),
    ("minecraft:wither_skeleton_skull", None, Some(EquipSlot::Head)),
    ("minecraft:wolf_armor", Some(1), Some(EquipSlot::Body)),
    ("minecraft:wooden_axe", Some(1), None),
    ("minecraft:wooden_hoe", Some(1), None),
    ("minecraft:wooden_pickaxe", Some(1), None),
    ("minecraft:wooden_shovel", Some(1), None),
    ("minecraft:wooden_spear", Some(1), None),
    ("minecraft:wooden_sword", Some(1), None),
    ("minecraft:writable_book", Some(1), None),
    ("minecraft:written_book", Some(16), None),
    ("minecraft:yellow_banner", Some(16), None),
    ("minecraft:yellow_bed", Some(1), None),
    ("minecraft:yellow_bundle", Some(1), None),
    ("minecraft:yellow_carpet", None, Some(EquipSlot::Body)),
    ("minecraft:yellow_harness", Some(1), Some(EquipSlot::Body)),
    ("minecraft:yellow_shulker_box", Some(1), None),
    ("minecraft:zombie_head", None, Some(EquipSlot::Head)),
];

fn lookup(name: &str) -> Option<(Option<i32>, Option<EquipSlot>)> {
    ITEM_PROPS
        .binary_search_by(|(n, _, _)| (*n).cmp(name))
        .ok()
        .map(|i| (ITEM_PROPS[i].1, ITEM_PROPS[i].2))
}

/// `stack.getMaxStackSize()` for an item name.
///
/// An item absent from the table takes [`DEFAULT_MAX_STACK`], which is correct
/// **only for a name that came out of the item registry** — the table lists
/// every real item that differs, so absence means "the default", not
/// "unknown". A name this build has never heard of is a different question,
/// and the caller must have failed to resolve it long before reaching here:
/// `Items::name` returns `None`, and the click path declines to predict rather
/// than guessing a cap.
pub fn max_stack_size(name: &str) -> i32 {
    lookup(name).and_then(|(s, _)| s).unwrap_or(DEFAULT_MAX_STACK)
}

/// The slot an item can be equipped into, or `None` if it carries no
/// `minecraft:equippable`.
///
/// `LivingEntity.isEquippableInSlot` treats that absence as **main hand only**,
/// so an item without the component is refused by every armour slot — which is
/// why this returns an `Option` rather than defaulting to anything.
pub fn equip_slot(name: &str) -> Option<EquipSlot> {
    lookup(name).and_then(|(_, q)| q)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary search is only correct if the generator emitted the table
    /// sorted.
    #[test]
    fn the_table_is_sorted() {
        assert!(ITEM_PROPS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    /// One item per stack-size bucket, pinned by hand from the report, so a
    /// regenerated table that collapsed to one number fails here.
    #[test]
    fn the_three_stack_buckets_are_present() {
        assert_eq!(max_stack_size("minecraft:dirt"), 64);
        assert_eq!(max_stack_size("minecraft:ender_pearl"), 16);
        assert_eq!(max_stack_size("minecraft:diamond_sword"), 1);
    }

    /// A registry item outside the table takes the default.
    #[test]
    fn an_item_outside_the_table_takes_the_default() {
        assert_eq!(max_stack_size("minecraft:stone"), DEFAULT_MAX_STACK);
    }

    /// The armour rule's inputs: a helmet names `head`, a shield names
    /// `offhand`, and dirt names nothing at all — which is what makes every
    /// armour slot refuse it.
    #[test]
    fn equippable_slots_are_read() {
        assert_eq!(equip_slot("minecraft:diamond_helmet"), Some(EquipSlot::Head));
        assert_eq!(equip_slot("minecraft:shield"), Some(EquipSlot::Offhand));
        assert_eq!(equip_slot("minecraft:dirt"), None);
    }

    /// A carved pumpkin is equippable **and** stacks to the default — it is in
    /// the table for the second reason only, which is the row a size-only
    /// table would have dropped.
    #[test]
    fn an_equippable_item_at_the_default_size_is_still_listed() {
        assert_eq!(max_stack_size("minecraft:carved_pumpkin"), DEFAULT_MAX_STACK);
        assert_eq!(equip_slot("minecraft:carved_pumpkin"), Some(EquipSlot::Head));
    }
}
